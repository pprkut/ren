// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-FileCopyrightText: Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-or-later AND MIT

//! Servo in the UI process with its frames shared as GPU textures (the
//! zero-copy frame path): each frame is blitted on the GPU from Servo's
//! OpenGL framebuffer into a Vulkan image that OpenGL imports through
//! `GL_EXT_memory_object_fd`, and that image is handed to Slint's
//! femtovg-wgpu renderer as a wgpu texture. Needs Slint on wgpu's Vulkan
//! backend.
//!
//! The Vulkan/OpenGL interop follows Slint's `examples/servo` (MIT,
//! `rendering_context/vulkan.rs`); unlike the example, the shared images
//! are kept and reused instead of allocated for every frame.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ash::vk;
use glow::HasContext;
use servo::RenderingContext;
use slint::Image;
use slint::wgpu_30::wgpu::{self, hal};

use super::browser::Browser;
use super::headless::HeadlessContext;
use super::{Engine, Event, FrameStats, Input, Size, TabId, Waker};

thread_local! {
    /// Slint's wgpu device, from its rendering notifier.
    static DEVICE: RefCell<Option<wgpu::Device>> = const { RefCell::new(None) };
}

/// Tells the engine which wgpu device Slint renders with.
pub fn set_device(device: wgpu::Device) {
    DEVICE.with(|d| *d.borrow_mut() = Some(device));
}

const GL_DEDICATED_MEMORY_OBJECT_EXT: u32 = 0x9581;
const GL_HANDLE_TYPE_OPAQUE_FD_EXT: u32 = 0x9586;

type CreateMemoryObjects = unsafe extern "system" fn(i32, *mut u32);
type MemoryObjectParameteriv = unsafe extern "system" fn(u32, u32, *const i32);
type ImportMemoryFd = unsafe extern "system" fn(u32, u64, u32, i32);
type TexStorageMem2D = unsafe extern "system" fn(u32, i32, u32, i32, i32, u32, u64);
type DeleteMemoryObjects = unsafe extern "system" fn(i32, *const u32);

/// The functions of `GL_EXT_memory_object` and `GL_EXT_memory_object_fd`,
/// which glow doesn't have.
struct MemoryObjectExt {
    create: CreateMemoryObjects,
    parameter: MemoryObjectParameteriv,
    import_fd: ImportMemoryFd,
    tex_storage_2d: TexStorageMem2D,
    delete: DeleteMemoryObjects,
}

impl MemoryObjectExt {
    fn load(context: &HeadlessContext) -> Option<Self> {
        let get = |name| {
            let f = context.proc_address(name);
            (!f.is_null()).then_some(f)
        };
        // SAFETY: the pointers are the named GL functions, whose
        // signatures the types follow.
        unsafe {
            Some(Self {
                create: std::mem::transmute::<*const c_void, CreateMemoryObjects>(get(
                    "glCreateMemoryObjectsEXT",
                )?),
                parameter: std::mem::transmute::<*const c_void, MemoryObjectParameteriv>(get(
                    "glMemoryObjectParameterivEXT",
                )?),
                import_fd: std::mem::transmute::<*const c_void, ImportMemoryFd>(get(
                    "glImportMemoryFdEXT",
                )?),
                tex_storage_2d: std::mem::transmute::<*const c_void, TexStorageMem2D>(get(
                    "glTexStorageMem2DEXT",
                )?),
                delete: std::mem::transmute::<*const c_void, DeleteMemoryObjects>(get(
                    "glDeleteMemoryObjectsEXT",
                )?),
            })
        }
    }
}

/// A Vulkan image that OpenGL draws into and Slint samples from.
struct SharedImage {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    gl_texture: glow::NativeTexture,
    framebuffer: glow::NativeFramebuffer,
    memory_object: u32,
}

pub struct Wgpu {
    browser: Browser,
    context: Rc<HeadlessContext>,
    device: wgpu::Device,
    ext: MemoryObjectExt,
    next_tab: TabId,
    /// Two images: Slint may still sample the one shown.
    images: [Option<SharedImage>; 2],
    next: usize,
    frame: Option<Image>,
    stats: FrameStats,
}

impl Wgpu {
    pub fn new(waker: Waker, size: Size, dark: bool) -> Result<Self, String> {
        let device = DEVICE
            .with(|d| d.borrow().clone())
            .ok_or("Slint doesn't render with wgpu (use --renderer femtovg-wgpu)")?;
        // SAFETY: only checks the backend.
        if unsafe { device.as_hal::<hal::api::Vulkan>() }.is_none() {
            return Err("Slint's wgpu doesn't use Vulkan".to_owned());
        }
        let context = Rc::new(
            HeadlessContext::new(dpi::PhysicalSize::new(size.width, size.height))
                .map_err(|err| format!("no OpenGL context for Servo: {err:?}"))?,
        );
        let ext = MemoryObjectExt::load(&context)
            .ok_or("OpenGL has no GL_EXT_memory_object_fd for sharing frames")?;
        Ok(Self {
            browser: Browser::new(waker, context.clone(), size, dark),
            context,
            device,
            ext,
            next_tab: 0,
            images: [None, None],
            next: 0,
            frame: None,
            stats: FrameStats::default(),
        })
    }

    fn share_frame(&mut self) -> Result<(), String> {
        let started = Instant::now();
        let size = self.context.size();
        let slot = self.next;
        if !matches!(&self.images[slot], Some(i) if i.width == size.width && i.height == size.height)
        {
            if let Some(old) = self.images[slot].take() {
                self.delete_gl(&old);
            }
            self.images[slot] = Some(self.create_image(size.width, size.height)?);
        }
        let image = self.images[slot].as_ref().expect("created above");
        let gl = self.context.glow_gl_api();
        let (w, h) = (size.width as i32, size.height as i32);
        // SAFETY: plain GL calls on the current context and our objects.
        unsafe {
            gl.bind_framebuffer(
                glow::READ_FRAMEBUFFER,
                std::num::NonZeroU32::new(self.context.framebuffer()).map(glow::NativeFramebuffer),
            );
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(image.framebuffer));
            // Flipped: OpenGL's first row is the bottom one.
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                0,
                h,
                w,
                0,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            // Without semaphores between OpenGL and Vulkan, wait until the
            // blit is done before Slint samples the image.
            gl.finish();
        }
        let texture = image.texture.clone();
        self.frame = Some(Image::try_from(texture).map_err(|err| err.to_string())?);
        self.next = 1 - slot;
        self.stats.add(started.elapsed());
        Ok(())
    }

    /// Creates a Vulkan image with exportable memory and imports it into
    /// OpenGL as the colour attachment of a framebuffer.
    fn create_image(&self, width: u32, height: u32) -> Result<SharedImage, String> {
        let vk_err = |err: vk::Result| format!("Vulkan: {err}");
        // SAFETY: Vulkan and GL calls with valid handles; the image and its
        // memory are freed by the wgpu texture's drop callback.
        unsafe {
            let hal_device = self
                .device
                .as_hal::<hal::api::Vulkan>()
                .ok_or("Slint's wgpu doesn't use Vulkan")?;
            let vk_device = hal_device.raw_device().clone();
            let instance = hal_device.shared_instance().raw_instance();

            let mut external_info = vk::ExternalMemoryImageCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let vk_image = vk_device
                .create_image(
                    &vk::ImageCreateInfo::default()
                        .image_type(vk::ImageType::TYPE_2D)
                        .format(vk::Format::R8G8B8A8_UNORM)
                        .extent(vk::Extent3D {
                            width,
                            height,
                            depth: 1,
                        })
                        .mip_levels(1)
                        .array_layers(1)
                        .samples(vk::SampleCountFlags::TYPE_1)
                        .tiling(vk::ImageTiling::OPTIMAL)
                        .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE)
                        .initial_layout(vk::ImageLayout::UNDEFINED)
                        .push_next(&mut external_info),
                    None,
                )
                .map_err(vk_err)?;

            let requirements = vk_device.get_image_memory_requirements(vk_image);
            let properties =
                instance.get_physical_device_memory_properties(hal_device.raw_physical_device());
            let memory_type = (0..properties.memory_type_count)
                .find(|&i| {
                    requirements.memory_type_bits & (1 << i) != 0
                        && properties.memory_types[i as usize]
                            .property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .ok_or("no device-local Vulkan memory for the image")?;
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(vk_image);
            let mut export = vk::ExportMemoryAllocateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let memory = vk_device
                .allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(requirements.size)
                        .memory_type_index(memory_type)
                        .push_next(&mut dedicated)
                        .push_next(&mut export),
                    None,
                )
                .map_err(vk_err)?;
            vk_device
                .bind_image_memory(vk_image, memory, 0)
                .map_err(vk_err)?;
            let fd = ash::khr::external_memory_fd::Device::new(instance, &vk_device)
                .get_memory_fd(
                    &vk::MemoryGetFdInfoKHR::default()
                        .memory(memory)
                        .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
                )
                .map_err(vk_err)?;

            // OpenGL takes over the file descriptor.
            let gl = self.context.glow_gl_api();
            self.context
                .make_current()
                .map_err(|err| format!("{err:?}"))?;
            let mut memory_object = 0;
            (self.ext.create)(1, &mut memory_object);
            (self.ext.parameter)(memory_object, GL_DEDICATED_MEMORY_OBJECT_EXT, &1);
            (self.ext.import_fd)(
                memory_object,
                requirements.size,
                GL_HANDLE_TYPE_OPAQUE_FD_EXT,
                fd,
            );
            let gl_texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(gl_texture));
            (self.ext.tex_storage_2d)(
                glow::TEXTURE_2D,
                1,
                glow::RGBA8,
                width as i32,
                height as i32,
                memory_object,
                0,
            );
            let framebuffer = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::DRAW_FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(gl_texture),
                0,
            );

            let extent = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let hal_texture = hal_device.texture_from_raw(
                vk_image,
                &hal::TextureDescriptor {
                    label: None,
                    size: extent,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    dimension: wgpu::TextureDimension::D2,
                    mip_level_count: 1,
                    sample_count: 1,
                    usage: wgpu::TextureUses::RESOURCE | wgpu::TextureUses::COLOR_TARGET,
                    view_formats: Vec::new(),
                    memory_flags: hal::MemoryFlags::empty(),
                },
                Some(Box::new(move || {
                    vk_device.destroy_image(vk_image, None);
                    vk_device.free_memory(memory, None);
                })),
                hal::vulkan::TextureMemory::External,
            );
            let texture = self.device.create_texture_from_hal::<hal::api::Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("servo frame"),
                    size: extent,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    dimension: wgpu::TextureDimension::D2,
                    mip_level_count: 1,
                    sample_count: 1,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                wgpu::TextureUses::UNINITIALIZED,
            );
            Ok(SharedImage {
                width,
                height,
                texture,
                gl_texture,
                framebuffer,
                memory_object,
            })
        }
    }

    /// Deletes the OpenGL side of an image; the Vulkan side goes with the
    /// last reference to its wgpu texture.
    fn delete_gl(&self, image: &SharedImage) {
        let gl = self.context.glow_gl_api();
        // SAFETY: our own objects, on our context.
        unsafe {
            let _ = self.context.make_current();
            gl.delete_framebuffer(image.framebuffer);
            gl.delete_texture(image.gl_texture);
            (self.ext.delete)(1, &image.memory_object);
        }
    }
}

impl Drop for Wgpu {
    fn drop(&mut self) {
        let images = std::mem::take(&mut self.images);
        for image in images.into_iter().flatten() {
            self.delete_gl(&image);
        }
    }
}

impl Engine for Wgpu {
    fn open(&mut self, url: &str) -> TabId {
        let tab = self.next_tab;
        self.next_tab += 1;
        self.browser.open(tab, url);
        tab
    }

    fn close(&mut self, tab: TabId) {
        self.browser.close(tab);
    }

    fn activate(&mut self, tab: Option<TabId>) {
        self.browser.activate(tab);
    }

    fn resize(&mut self, size: Size) {
        self.browser.resize(size);
    }

    fn input(&mut self, input: Input) {
        self.browser.input(input);
    }

    fn set_dark(&mut self, dark: bool) {
        self.browser.set_dark(dark);
    }

    fn pump(&mut self) -> Vec<Event> {
        let mut events = self.browser.spin();
        if self.browser.paint()
            && let Err(err) = self.share_frame()
        {
            events.push(Event::Failed(format!("sharing a frame failed: {err}")));
        }
        events
    }

    fn take_frame(&mut self) -> Option<Image> {
        self.frame.take()
    }

    fn frame_stats(&self) -> Option<(usize, Duration, Duration)> {
        self.stats.summary()
    }
}

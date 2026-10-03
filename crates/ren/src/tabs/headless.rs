// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: GPL-3.0-or-later

//! An offscreen OpenGL context on the GPU for Servo, whose frames are read
//! back into memory: the CPU readback frame path. Unlike Servo's
//! `SoftwareRenderingContext` it uses the hardware adapter, and unlike its
//! `WindowRenderingContext` it needs no window, so it works with any Slint
//! renderer and in a helper process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::default::Size2D;
use gleam::gl::{self, Gl};
use servo::{DeviceIntRect, RenderingContext, RgbaImage};
use surfman::{
    Connection, Context, ContextAttributeFlags, ContextAttributes, Device, Error, GLApi, GLVersion,
    SurfaceAccess, SurfaceType,
};

pub struct HeadlessContext {
    size: Cell<PhysicalSize<u32>>,
    device: RefCell<Device>,
    context: RefCell<Context>,
    gleam: Rc<dyn Gl>,
    glow: Arc<glow::Context>,
}

impl HeadlessContext {
    pub fn new(size: PhysicalSize<u32>) -> Result<Self, Error> {
        let connection = Connection::new()?;
        let adapter = connection.create_adapter()?;
        let mut device = connection.create_device(&adapter)?;
        let version = match connection.gl_api() {
            GLApi::GLES => GLVersion { major: 3, minor: 0 },
            GLApi::GL => GLVersion { major: 3, minor: 2 },
        };
        let flags = ContextAttributeFlags::ALPHA
            | ContextAttributeFlags::DEPTH
            | ContextAttributeFlags::STENCIL;
        let descriptor = device.create_context_descriptor(&ContextAttributes { flags, version })?;
        let mut context = device.create_context(&descriptor, None)?;
        bind_new_surface(&mut device, &mut context, size)?;
        device.make_context_current(&context)?;

        // SAFETY: the loaders only look up GL functions of this context.
        let gleam = unsafe {
            match connection.gl_api() {
                GLApi::GL => gl::GlFns::load_with(|f| device.get_proc_address(&context, f)),
                GLApi::GLES => gl::GlesFns::load_with(|f| device.get_proc_address(&context, f)),
            }
        };
        // SAFETY: as above.
        let glow = unsafe {
            glow::Context::from_loader_function(|f| device.get_proc_address(&context, f))
        };
        Ok(Self {
            size: Cell::new(size),
            device: RefCell::new(device),
            context: RefCell::new(context),
            gleam,
            glow: Arc::new(glow),
        })
    }

    fn framebuffer(&self) -> u32 {
        self.device
            .borrow()
            .context_surface_info(&self.context.borrow())
            .ok()
            .flatten()
            .and_then(|info| info.framebuffer_object)
            .map_or(0, |fbo| fbo.0.get())
    }

    /// Reads the current frame as RGBA, top row first, into `out`, which
    /// must hold `width * height * 4` bytes.
    pub fn read_into(&self, out: &mut [u8]) {
        let size = self.size.get();
        let gl = &self.gleam;
        gl.bind_framebuffer(gl::FRAMEBUFFER, self.framebuffer());
        gl.bind_vertex_array(0);
        gl.read_pixels_into_buffer(
            0,
            0,
            size.width as i32,
            size.height as i32,
            gl::RGBA,
            gl::UNSIGNED_BYTE,
            out,
        );
        // OpenGL's first row is the bottom one.
        let stride = size.width as usize * 4;
        let rows = size.height as usize;
        for y in 0..rows / 2 {
            let (top, bottom) = out.split_at_mut((rows - 1 - y) * stride);
            top[y * stride..(y + 1) * stride].swap_with_slice(&mut bottom[..stride]);
        }
    }
}

fn bind_new_surface(
    device: &mut Device,
    context: &mut Context,
    size: PhysicalSize<u32>,
) -> Result<(), Error> {
    let size = Size2D::new(size.width.max(1) as i32, size.height.max(1) as i32);
    let surface = device.create_surface(
        context,
        SurfaceAccess::GPUOnly,
        SurfaceType::Generic { size },
    )?;
    device
        .bind_surface_to_context(context, surface)
        .map_err(|(err, mut surface)| {
            let _ = device.destroy_surface(context, &mut surface);
            err
        })
}

impl Drop for HeadlessContext {
    fn drop(&mut self) {
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        if let Ok(Some(mut surface)) = device.unbind_surface_from_context(context) {
            let _ = device.destroy_surface(context, &mut surface);
        }
        let _ = device.destroy_context(context);
    }
}

impl RenderingContext for HeadlessContext {
    fn prepare_for_rendering(&self) {
        self.gleam
            .bind_framebuffer(gl::FRAMEBUFFER, self.framebuffer());
    }

    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage> {
        // Servo only asks for this for screenshots; frames go through
        // `read_into`.
        let size = self.size.get();
        let mut pixels = vec![0; size.width as usize * size.height as usize * 4];
        self.read_into(&mut pixels);
        let rect = rect
            .to_usize()
            .intersection(&euclid::Box2D::from_size(euclid::Size2D::new(
                size.width as usize,
                size.height as usize,
            )))?;
        let stride = size.width as usize * 4;
        let mut cropped = Vec::with_capacity(rect.area() * 4);
        for row in rect.y_range() {
            cropped.extend_from_slice(&pixels[row * stride + rect.min.x * 4..][..rect.width() * 4]);
        }
        RgbaImage::from_raw(rect.width() as u32, rect.height() as u32, cropped)
    }

    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if self.size.get() == size || size.width == 0 || size.height == 0 {
            return;
        }
        let device = &mut self.device.borrow_mut();
        let context = &mut self.context.borrow_mut();
        if let Ok(Some(mut surface)) = device.unbind_surface_from_context(context) {
            let _ = device.destroy_surface(context, &mut surface);
        }
        match bind_new_surface(device, context, size) {
            Ok(()) => self.size.set(size),
            Err(err) => eprintln!("ren: resizing the page surface failed: {err:?}"),
        }
    }

    fn present(&self) {}

    fn make_current(&self) -> Result<(), Error> {
        self.device
            .borrow()
            .make_context_current(&self.context.borrow())
    }

    fn gleam_gl_api(&self) -> Rc<dyn Gl> {
        self.gleam.clone()
    }

    fn glow_gl_api(&self) -> Arc<glow::Context> {
        self.glow.clone()
    }

    fn connection(&self) -> Option<Connection> {
        Some(self.device.borrow().connection())
    }
}

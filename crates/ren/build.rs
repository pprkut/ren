// SPDX-FileCopyrightText: Copyright 2026  Heinz Wiesinger, Amsterdam, The Netherlands
// SPDX-License-Identifier: CC0-1.0

fn main() {
    let mut config = slint_build::CompilerConfiguration::new();
    if std::env::var_os("CARGO_FEATURE_STYLE_QT").is_some() {
        config = config.with_style("qt".to_owned());
    }
    slint_build::compile_with_config("src/ui/main_window.slint", config)
        .expect("Slint build failed");
}

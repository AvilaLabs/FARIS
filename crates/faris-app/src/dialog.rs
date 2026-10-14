//! File dialogs. The desktop uses the system dialog. The browser cannot show a
//! blocking dialog; the panels that need one are desktop-only there, so this
//! stand-in picks nothing. Opening and saving studies in the browser use the
//! page's own picker and download (see `web.rs`).

#[cfg(not(target_arch = "wasm32"))]
pub use rfd::FileDialog;

#[cfg(target_arch = "wasm32")]
pub use browser::FileDialog;

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::path::{Path, PathBuf};

    #[derive(Default)]
    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            Self
        }
        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }
        pub fn add_filter(self, _name: impl Into<String>, _extensions: &[impl ToString]) -> Self {
            self
        }
        pub fn set_directory(self, _directory: impl AsRef<Path>) -> Self {
            self
        }
        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }
        pub fn pick_file(self) -> Option<PathBuf> {
            None
        }
        pub fn pick_folder(self) -> Option<PathBuf> {
            None
        }
        pub fn save_file(self) -> Option<PathBuf> {
            None
        }
    }
}

pub mod traits;

#[cfg(feature = "real-fs")]
pub mod real_fs;

#[cfg(feature = "real-fs")]
pub mod walk;

#[cfg(feature = "mock-fs")]
pub mod mock_fs;

pub use traits::*;

#[cfg(feature = "real-fs")]
pub use real_fs::*;

#[cfg(feature = "real-fs")]
pub use walk::codemod_walk_builder;

#[cfg(feature = "mock-fs")]
pub use mock_fs::*;

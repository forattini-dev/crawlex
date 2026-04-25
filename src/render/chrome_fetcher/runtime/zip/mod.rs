#[cfg(all(feature = "zip0", not(feature = "zip8")))]
mod zip0;
#[cfg(all(feature = "zip0", not(feature = "zip8")))]
pub use zip0::ZipArchive;

#[cfg(feature = "zip8")]
mod zip8;
#[cfg(feature = "zip8")]
pub use zip8::ZipArchive;

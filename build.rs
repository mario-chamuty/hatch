use std::io;
use std::fs::File;
use std::path::Path;

#[cfg(windows)]
use winres::WindowsResource;

#[cfg(windows)]
use image::{DynamicImage, ImageFormat};

fn main() -> io::Result<()> {
    #[cfg(windows)]
    {
        // Convert PNG to ICO if it doesn't exist
        let ico_path = Path::new("icon.ico");
        if !ico_path.exists() {
            if let Ok(img) = image::open("icon.png") {
                create_ico_from_png(img, "icon.ico").ok();
            }
        }

        // Set the icon for Windows executable
        WindowsResource::new()
            .set_icon("icon.ico")
            .compile()?;
    }

    // Embed the icon as raw bytes for runtime use
    println!("cargo:rerun-if-changed=icon.png");

    Ok(())
}

#[cfg(windows)]
fn create_ico_from_png(img: DynamicImage, output_path: &str) -> io::Result<()> {
    use std::io::Write;

    // ICO header
    let mut ico_data = Vec::new();

    // ICO file header
    ico_data.extend_from_slice(&[0, 0]); // Reserved
    ico_data.extend_from_slice(&[1, 0]); // Type (1 = ICO)
    ico_data.extend_from_slice(&[1, 0]); // Number of images

    // Create 256x256 PNG for the ICO
    let resized = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3);
    let mut png_data = Vec::new();
    resized.write_to(&mut std::io::Cursor::new(&mut png_data), ImageFormat::Png)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    // ICO directory entry
    ico_data.push(0); // Width (0 means 256)
    ico_data.push(0); // Height (0 means 256)
    ico_data.push(0); // Color palette
    ico_data.push(0); // Reserved
    ico_data.extend_from_slice(&[1, 0]); // Color planes
    ico_data.extend_from_slice(&[32, 0]); // Bits per pixel
    let size = png_data.len() as u32;
    ico_data.extend_from_slice(&size.to_le_bytes()); // Size of image data
    let offset = 22u32; // Offset to image data (header + directory)
    ico_data.extend_from_slice(&offset.to_le_bytes());

    // Append PNG data
    ico_data.extend_from_slice(&png_data);

    // Write ICO file
    let mut file = File::create(output_path)?;
    file.write_all(&ico_data)?;

    Ok(())
}
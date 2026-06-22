fn main() {
    let mut cb = arboard::Clipboard::new().unwrap();
    match cb.get_image() {
        Ok(img) => println!("Success: {}x{}", img.width, img.height),
        Err(e) => println!("Error: {:?}", e)
    }
}

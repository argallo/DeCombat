// out in x y w h size
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n = |i: usize| a[i].parse::<u32>().unwrap();
    let i = image::open(&a[2]).unwrap().to_rgb8();
    let c = image::imageops::crop_imm(&i, n(3), n(4), n(5), n(6)).to_image();
    image::imageops::resize(&c, n(7), n(7), image::imageops::FilterType::CatmullRom).save(&a[1]).unwrap();
}

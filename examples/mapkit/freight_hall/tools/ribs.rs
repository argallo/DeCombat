// ribbed painted steel from SimReady pictures: out rib mottle rust tint rust_amount rib_gain [gain]
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let load = |p: &str| image::open(p).unwrap().to_rgb8();
    let (rib, mottle, rust, tint) = (load(&a[2]), load(&a[3]), load(&a[4]), load(&a[5]));
    let amount: f32 = a[6].parse().unwrap();
    let gain: f32 = a[7].parse().unwrap();
    let bright: f32 = a.get(8).map_or(1.0, |s| s.parse().unwrap());
    let lum = |p: &image::Rgb<u8>| (p[0] as f32 + p[1] as f32 + p[2] as f32) / 765.0;
    let mean = |i: &image::RgbImage| i.pixels().map(|p| lum(p)).sum::<f32>() / (i.width() * i.height()) as f32;
    let (mr, mm) = (mean(&rib), mean(&mottle));
    let t0 = tint.get_pixel(0, 0);
    let t = [0, 1, 2].map(|k| t0[k] as f32 * 0.62 + 44.0);
    let (w, h) = (rib.width(), rib.height());
    let mut out = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (1.0 + gain * (lum(rib.get_pixel(x, y)) / mr - 1.0)).clamp(0.55, 1.4);
            let m = (lum(mottle.get_pixel(x % mottle.width(), y % mottle.height())) / mm).clamp(0.7, 1.3);
            let s = rust.get_pixel(x % rust.width(), y % rust.height());
            let red = ((s[0] as f32 - s[2] as f32) / 90.0).clamp(0.0, 1.0) * amount;
            let c = [0, 1, 2].map(|k| {
                let paint = t[k] * m * bright;
                ((paint * (1.0 - red) + s[k] as f32 * 0.62 * red) * r).clamp(0.0, 255.0) as u8
            });
            out.put_pixel(x, y, image::Rgb(c));
        }
    }
    out.save(&a[1]).unwrap();
}

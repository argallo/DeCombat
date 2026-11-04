//! Compares two folders of PNGs of the same names, pixel by pixel.
//!
//! ```text
//! cargo run --release -p dc-client --example png_diff -- <dir A> <dir B> [--diff-out DIR]
//! ```
//! For each file present in both: pixels compared, pixels that differ, the largest channel difference, a
//! histogram of the per-pixel largest difference (1, 2, 3, 4-8, 9-32, above 32) and the bounding box of the
//! differing pixels. With `--diff-out` an amplified difference image is written per differing file. Exit code 1
//! when any file differs by more than 2 on a channel.

use std::path::{Path, PathBuf};

fn load(p: &Path) -> Option<image::RgbaImage> {
    image::open(p).ok().map(|i| i.to_rgba8())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: png_diff <dir A> <dir B> [--diff-out DIR]");
        std::process::exit(2);
    }
    let (a, b) = (PathBuf::from(&args[0]), PathBuf::from(&args[1]));
    let diff_out = args
        .iter()
        .position(|x| x == "--diff-out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let mut names: Vec<String> = std::fs::read_dir(&a)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.to_ascii_lowercase().ends_with(".png"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let (mut total_px, mut total_diff, mut worst) = (0u64, 0u64, 0u8);
    let mut hist_all = [0u64; 6];
    let mut bad = false;
    println!("file | pixels | differing | max delta | hist d=1/2/3/4-8/9-32/>32 | bbox");
    for n in &names {
        let (Some(x), Some(y)) = (load(&a.join(n)), load(&b.join(n))) else {
            println!("{n} | missing in B or unreadable");
            bad = true;
            continue;
        };
        if x.dimensions() != y.dimensions() {
            println!("{n} | size {:?} vs {:?}", x.dimensions(), y.dimensions());
            bad = true;
            continue;
        }
        let (w, h) = x.dimensions();
        let mut hist = [0u64; 6];
        let (mut diff, mut maxd) = (0u64, 0u8);
        let mut bbox = [u32::MAX, u32::MAX, 0, 0];
        let mut amp = diff_out
            .as_ref()
            .map(|_| image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255])));
        for (px, (p, q)) in x.pixels().zip(y.pixels()).enumerate() {
            let d = (0..4).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
            if d == 0 {
                continue;
            }
            diff += 1;
            maxd = maxd.max(d);
            let bin = match d {
                1 => 0,
                2 => 1,
                3 => 2,
                4..=8 => 3,
                9..=32 => 4,
                _ => 5,
            };
            hist[bin] += 1;
            let (xx, yy) = (px as u32 % w, px as u32 / w);
            bbox = [
                bbox[0].min(xx),
                bbox[1].min(yy),
                bbox[2].max(xx),
                bbox[3].max(yy),
            ];
            if let Some(img) = amp.as_mut() {
                let v = (d as u32 * 32).min(255) as u8;
                img.put_pixel(xx, yy, image::Rgba([v, v, v, 255]));
            }
        }
        total_px += (w * h) as u64;
        total_diff += diff;
        worst = worst.max(maxd);
        for (t, v) in hist_all.iter_mut().zip(hist) {
            *t += v;
        }
        bad |= maxd > 2;
        let bb = if diff > 0 {
            format!("{:?}", bbox)
        } else {
            "-".into()
        };
        println!(
            "{n} | {} | {diff} | {maxd} | {}/{}/{}/{}/{}/{} | {bb}",
            w * h,
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            hist[4],
            hist[5]
        );
        if let (Some(dir), Some(img), true) = (&diff_out, amp, diff > 0) {
            let _ = std::fs::create_dir_all(dir);
            let _ = img.save(dir.join(n));
        }
    }
    println!(
        "TOTAL | {total_px} | {total_diff} | {worst} | {}/{}/{}/{}/{}/{} | files {}",
        hist_all[0],
        hist_all[1],
        hist_all[2],
        hist_all[3],
        hist_all[4],
        hist_all[5],
        names.len()
    );
    if bad {
        std::process::exit(1);
    }
}

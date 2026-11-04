//! Generate small, reproducible inputs for exercising the CLI and viewer.
use image::{Rgba, RgbaImage};
fn main() -> anyhow::Result<()> {
    std::fs::create_dir_all("examples/inputs")?;
    let mut image = RgbaImage::new(128, 128);
    for y in 0..128 {
        for x in 0..128 {
            let p = if (24..104).contains(&x) && (57..116).contains(&y) {
                Some([225, 181, 95, 255])
            } else if (17..112).contains(&x)
                && (14..58).contains(&y)
                && (x as i32 - 64).abs() < (y as i32 - 10)
            {
                Some([190, 56, 51, 255])
            } else {
                None
            };
            if let Some(p) = p {
                image.put_pixel(x, y, Rgba(p));
            }
            if (56..75).contains(&x) && (83..116).contains(&y) {
                image.put_pixel(x, y, Rgba([83, 56, 40, 255]));
            }
            if ((34..49).contains(&x) || (81..96).contains(&x)) && (73..88).contains(&y) {
                image.put_pixel(x, y, Rgba([75, 163, 211, 255]));
            }
        }
    }
    image.save("examples/inputs/house.png")?;
    let mut jpg = image::RgbImage::new(128, 128);
    for (x, y, p) in jpg.enumerate_pixels_mut() {
        let c = image.get_pixel(x, y);
        *p = image::Rgb(if c[3] == 0 {
            [235, 239, 245]
        } else {
            [c[0], c[1], c[2]]
        });
    }
    jpg.save("examples/inputs/house.jpg")?;
    std::fs::write(
        "examples/inputs/house.mtl",
        "newmtl wall\nKd 0.88 0.71 0.37\nnewmtl roof\nKd 0.75 0.22 0.20\n",
    )?;
    std::fs::write(
        "examples/inputs/house.obj",
        "mtllib house.mtl\nv -2 0 -2\nv 2 0 -2\nv 2 3 -2\nv -2 3 -2\nv -2 0 2\nv 2 0 2\nv 2 3 2\nv -2 3 2\nv 0 5 -2\nv 0 5 2\nusemtl wall\nf 1 4 3 2\nf 5 6 7 8\nf 1 2 6 5\nf 1 5 8 4\nf 2 3 7 6\nusemtl roof\nf 4 9 3\nf 8 7 10\nf 4 8 10 9\nf 3 9 10 7\n",
    )?;
    Ok(())
}

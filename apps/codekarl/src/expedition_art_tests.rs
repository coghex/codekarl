//! The approved expedition chip pixels (trade part G).

#[test]
fn shipped_chips_match_the_approved_art() {
    for (name, expected) in [
        ("covered_wagon", 0xd181_17a1_ba6c_0da6),
        ("currach", 0xe518_3605_f17e_0e58),
        ("hand_cart", 0x8aa5_3973_2855_ea68),
        ("on_foot", 0xda3e_fe2c_d054_66c0),
        ("ox_cart", 0xb7a3_276a_465c_e30e),
        ("simple_barge", 0xf85e_011e_3b4b_27e6),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ui/expeditions")
            .join(format!("{name}.png"));
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (21, 22, png::ColorType::Rgba)
        );
        let hash = pixels[..info.buffer_size()]
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, byte| {
                (h ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        assert_eq!(hash, expected, "{name}");
    }
}

#[test]
fn title_icons_preserve_the_approved_shapes_as_tintable_pixels() {
    for (name, expected) in [
        ("covered_wagon", 0x8063_4d4c_0935_db69),
        ("currach", 0x5855_acb6_3d04_6a14),
        ("hand_cart", 0xe2a7_12eb_f14b_0904),
        ("on_foot", 0x1e85_5e61_f0bd_8574),
        ("ox_cart", 0x248c_00c0_d043_42f1),
        ("simple_barge", 0x55ba_d8df_e45f_e261),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/ui/expeditions")
            .join(format!("title_{name}.png"));
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(
            (info.width, info.height, info.color_type),
            (13, 11, png::ColorType::Rgba)
        );
        let hash = pixels[..info.buffer_size()]
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, byte| {
                (h ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        assert_eq!(hash, expected, "{name}");
    }
}

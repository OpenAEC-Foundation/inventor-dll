//! Colours from what the file says about them in words. The colours of the appearances
//! themselves (the asset library inside the file) are not read; an appearance whose name holds
//! its colour (`Opaque(67,72,77)` or `178,125,71`, as converted models have them, or a plain
//! colour word in English, German or Dutch) gives that colour.

/// Red, green and blue from 0 to 1.
pub type Rgb = [f32; 3];

/// `r,g,b` with each from 0 to 255, as the face colours of the B-rep are written.
pub fn from_triple(text: &str) -> Option<Rgb> {
    let mut parts = text.split(',').map(|p| p.trim().parse::<u8>());
    let rgb = [parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?];
    parts.next().is_none().then(|| rgb.map(|v| f32::from(v) / 255.0))
}

/// The colour an appearance name tells, if it tells one.
pub fn from_name(name: &str) -> Option<Rgb> {
    // nothing but the colour: 178,125,71
    if let Some(rgb) = from_triple(name) {
        return Some(rgb);
    }
    // three numbers from 0 to 255 after each other: (67,72,77), RGB-51-51-51, 255 128 0
    let numbers: Vec<&str> = name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).collect();
    if numbers.len() == 3 && name.chars().any(|c| matches!(c, '(' | ',' | '-' | ' ')) {
        if let [Ok(r), Ok(g), Ok(b)] = [numbers[0].parse::<u8>(), numbers[1].parse::<u8>(), numbers[2].parse::<u8>()] {
            let lower = name.to_lowercase();
            if lower.contains("rgb") || lower.contains("opaque") || name.contains('(') {
                return Some([r, g, b].map(|v| f32::from(v) / 255.0));
            }
        }
    }
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).collect();
    const COLOURS: [(&[&str], [u8; 3]); 14] = [
        (&["black", "schwarz", "zwart"], [35, 35, 38]),
        (&["white", "weiß", "weiss", "wit"], [240, 240, 238]),
        (&["red", "rot", "rood"], [200, 40, 40]),
        (&["green", "grün", "groen"], [50, 150, 70]),
        (&["blue", "blau", "blauw"], [45, 90, 190]),
        (&["yellow", "gelb", "geel"], [235, 205, 50]),
        (&["orange", "oranje"], [235, 130, 35]),
        (&["pink", "rosa", "roze"], [240, 150, 180]),
        (&["purple", "violet", "violett", "lila", "paars"], [130, 70, 170]),
        (&["brown", "braun", "bruin"], [120, 80, 50]),
        (&["grey", "gray", "grau", "grijs"], [140, 142, 146]),
        (&["ivory", "eierschale", "eggshell", "ivoor", "beige"], [238, 232, 210]),
        (&["gold", "brass", "messing"], [205, 170, 80]),
        (&["copper", "kupfer", "koper"], [190, 115, 75]),
    ];
    COLOURS.iter().find(|(names, _)| words.iter().any(|w| names.contains(w))).map(|(_, rgb)| rgb.map(|v| f32::from(v) / 255.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_from_text() {
        assert_eq!(from_triple("255,0,51"), Some([1.0, 0.0, 0.2]));
        assert_eq!(from_triple("84, 86, 89").map(|c| (c[0] * 255.0).round()), Some(84.0));
        assert_eq!(from_triple("1,2"), None);
        assert_eq!(from_triple("1,2,300"), None);
        assert_eq!(from_name("Opaque(255,0,51)"), Some([1.0, 0.0, 0.2]));
        assert_eq!(from_name("255,0,51"), Some([1.0, 0.0, 0.2]));
        assert_eq!(from_name("PSOLRGB-51-51-51"), Some([0.2, 0.2, 0.2]));
        assert_eq!(from_name("Glatt - Weiß"), Some([240.0 / 255.0, 240.0 / 255.0, 238.0 / 255.0]));
        assert_eq!(from_name("Rubber - Black"), from_name("Acetalharz, Schwarz"));
        assert!(from_name("Rot").is_some() && from_name("Smooth - RW Blue 1").is_some());
        // no colour in the name
        assert_eq!(from_name("Steel - Polished"), None);
        assert_eq!(from_name("Aluminium 6061"), None);
        assert_eq!(from_name("Default"), None);
        // "red" inside another word is not red
        assert_eq!(from_name("Predefined"), None);
    }
}

use serde_json::Value;

pub(super) fn json(value: &Value) -> String {
    let mut output = String::new();
    for character in value.to_string().chars() {
        let code = u32::from(character);
        if character == '`'
            || character.is_control()
            || matches!(code,0x00ad|0x034f|0x061c|0x180e|0xfeff|0x200b..=0x200f|0x202a..=0x202e|0x2060..=0x206f|0xfff9..=0xfffb|0xe0000..=0xe007f)
        {
            use std::fmt::Write as _;
            if code <= 0xffff {
                let _ = write!(output, "\\u{code:04x}");
            } else {
                let pair = code - 0x10000;
                let _ = write!(
                    output,
                    "\\u{:04x}\\u{:04x}",
                    0xd800 + (pair >> 10),
                    0xdc00 + (pair & 0x3ff)
                );
            }
        } else {
            output.push(character);
        }
    }
    output
}

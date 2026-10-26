//! Conservative macOS memory-pressure probing with an injectable explicit signal.

use super::MemoryPressureLevel;

const WARNING_PERCENT: u8 = 15;
const CRITICAL_PERCENT: u8 = 8;

pub(super) fn system_pressure() -> MemoryPressureLevel {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/bin/memory_pressure")
            .arg("-Q")
            .env_clear()
            .output();
        if let Ok(output) = output
            && output.status.success()
            && let Some(percent) = parse_free_percentage(&output.stdout)
        {
            return classify(percent);
        }
    }
    MemoryPressureLevel::Normal
}

const fn classify(free_percent: u8) -> MemoryPressureLevel {
    if free_percent <= CRITICAL_PERCENT {
        MemoryPressureLevel::Critical
    } else if free_percent <= WARNING_PERCENT {
        MemoryPressureLevel::Warning
    } else {
        MemoryPressureLevel::Normal
    }
}

fn parse_free_percentage(output: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(output).ok()?;
    let marker = "System-wide memory free percentage:";
    let remainder = text.split_once(marker)?.1.trim_start();
    let digits = remainder
        .bytes()
        .take_while(u8::is_ascii_digit)
        .collect::<Vec<_>>();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_classifies_memory_pressure_output() {
        assert_eq!(
            parse_free_percentage(b"System-wide memory free percentage: 42%\n"),
            Some(42)
        );
        assert_eq!(classify(42), MemoryPressureLevel::Normal);
        assert_eq!(classify(15), MemoryPressureLevel::Warning);
        assert_eq!(classify(8), MemoryPressureLevel::Critical);
        assert_eq!(parse_free_percentage(b"unknown"), None);
    }
}

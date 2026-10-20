//! Count-only tray presentation. The website title body is discarded immediately.

pub fn unread_count(title: &str) -> Option<u32> {
    let value = title.strip_prefix('(')?;
    let end = value.find(')')?;
    let digits = &value[..end];
    let tail = &value[end + 1..];
    if digits.is_empty()
        || digits.len() > 9
        || !digits.bytes().all(|value| value.is_ascii_digit())
        || tail
            .chars()
            .next()
            .is_some_and(|value| !value.is_whitespace())
    {
        return None;
    }
    digits.parse().ok()
}

pub fn next_unread(current: usize, counts: [Option<u32>; 2]) -> Option<usize> {
    let other = 1 - current.min(1);
    [other, current.min(1)]
        .into_iter()
        .find(|index| counts[*index].unwrap_or(0) > 0)
}

fn truncate_utf16(value: &str, limit: usize) -> String {
    if value.encode_utf16().count() <= limit {
        return value.into();
    }
    let mut output = String::new();
    let mut used = 0;
    for character in value.chars() {
        if used + character.len_utf16() + 1 > limit {
            break;
        }
        output.push(character);
        used += character.len_utf16();
    }
    output.push('…');
    output
}

pub fn tooltip(labels: &[String; 2], counts: [Option<u32>; 2]) -> String {
    let counts = counts.map(|count| count.map_or_else(|| "—".into(), |count| count.to_string()));
    let overhead = format!("Switchboard · : {} · : {}", counts[0], counts[1])
        .encode_utf16()
        .count();
    let budget = (127usize.saturating_sub(overhead)) / 2;
    format!(
        "Switchboard · {}: {} · {}: {}",
        truncate_utf16(&labels[0], budget),
        counts[0],
        truncate_utf16(&labels[1], budget),
        counts[1]
    )
}

pub fn icon(base: &tauri::image::Image<'_>, mask: u8) -> tauri::image::Image<'static> {
    let (width, height) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    let radius = f64::from(width.min(height)) * 0.21;
    let border = (f64::from(width.min(height)) / 32.0).max(1.0);
    for (index, center) in [0.28, 0.72].into_iter().enumerate() {
        let color = match (index, mask & (1 << index) != 0) {
            (0, true) => [182, 154, 247, 255],
            (0, false) => [78, 69, 102, 255],
            (_, true) => [237, 189, 114, 255],
            (_, false) => [103, 85, 58, 255],
        };
        for y in 0..height {
            for x in 0..width {
                let distance = ((f64::from(x) - f64::from(width) * center).powi(2)
                    + (f64::from(y) - f64::from(height) * 0.77).powi(2))
                .sqrt();
                if distance <= radius {
                    let pixel = ((y * width + x) * 4) as usize;
                    let color = if distance > radius - border {
                        [16, 18, 22, 255]
                    } else {
                        color
                    };
                    rgba[pixel..pixel + 4].copy_from_slice(&color);
                }
            }
        }
    }
    tauri::image::Image::new_owned(rgba, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_leading_bounded_ascii_count_is_used_from_the_website_title() {
        assert_eq!(unread_count("(12) WhatsApp"), Some(12));
        assert_eq!(unread_count("(0)"), Some(0));
        for title in [
            "WhatsApp",
            "name (3)",
            " (3) WhatsApp",
            "(3)name",
            "(9999999999) WhatsApp",
            "(٣) WhatsApp",
            "(3+) WhatsApp",
        ] {
            assert_eq!(unread_count(title), None);
        }
    }

    #[test]
    fn unread_navigation_cycles_other_account_and_handles_one_or_none() {
        assert_eq!(next_unread(0, [Some(2), Some(1)]), Some(1));
        assert_eq!(next_unread(1, [Some(2), Some(1)]), Some(0));
        assert_eq!(next_unread(0, [Some(2), None]), Some(0));
        assert_eq!(next_unread(1, [None, Some(1)]), Some(1));
        assert_eq!(next_unread(0, [None, Some(0)]), None);
    }

    #[test]
    fn tray_tooltip_keeps_both_counts_with_unicode_labels_inside_windows_limit() {
        let value = tooltip(
            &["🟣".repeat(28), "🟠".repeat(28)],
            [Some(123456789), Some(42)],
        );
        assert!(value.encode_utf16().count() <= 127);
        assert!(value.contains(": 123456789"));
        assert!(value.ends_with(": 42"));
    }

    #[test]
    fn tray_dots_map_personal_purple_and_work_amber_without_changing_image_size() {
        let base = tauri::image::Image::new_owned(vec![0; 32 * 32 * 4], 32, 32);
        let bright = icon(&base, 3);
        let muted = icon(&base, 0);
        let pixel = |image: &tauri::image::Image<'_>, x: usize| {
            image.rgba()[((25 * 32 + x) * 4)..((25 * 32 + x) * 4 + 4)].to_vec()
        };
        assert_eq!(pixel(&bright, 9), [182, 154, 247, 255]);
        assert_eq!(pixel(&bright, 23), [237, 189, 114, 255]);
        assert_ne!(pixel(&bright, 9), pixel(&muted, 9));
        assert_eq!(bright.rgba().len(), base.rgba().len());
        assert!(base.rgba().iter().all(|value| *value == 0));
    }
}

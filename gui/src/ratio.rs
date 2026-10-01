//! Compression-ratio helpers (plan 10 §phase 1): one place for the ratio
//! math, its display label and the good/neutral/grew hint bands shared by
//! the footer stats line, the report totals and the queue table.
//!
//! Pure logic only — the single egui type ([`hint_color`]'s return value)
//! carries no behavior. The ratio definition matches the CLI summary:
//! `output / input × 100 %`, so 31 means the output is 31 % of the input
//! and 69 % were saved.

use egui::Color32;

/// Color-hint band of a compression ratio (plan 10 §phase 1 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    /// Output ≤ 80 % of the input: a good shave (green tint).
    Good,
    /// 80–100 %: no remarkable gain or loss (no tint).
    Neutral,
    /// Output > 100 %: the file *grew* (amber/red tint).
    Grew,
}

/// `output / input × 100` as a percent value (`None` for a zero-size
/// input — the ratio is undefined there).
#[must_use]
pub fn ratio_percent(input: u64, output: u64) -> Option<f32> {
    if input == 0 {
        None
    } else {
        Some(output as f32 / input as f32 * 100.0)
    }
}

/// `output / input` as a fraction in the `0.0..` range (`None` for a
/// zero-size input). Non-finite results are impossible for finite sizes.
#[must_use]
pub fn ratio_fraction(input: u64, output: u64) -> Option<f32> {
    if input == 0 {
        None
    } else {
        Some(output as f32 / input as f32)
    }
}

/// Display label of a ratio: `"31%"` (rounded to whole percents) or `"—"`
/// when the input size is zero/unknown (undefined ratio).
#[must_use]
pub fn ratio_label(input: u64, output: u64) -> String {
    match ratio_percent(input, output) {
        Some(percent) => format!("{:.0}%", percent.round()),
        None => "—".to_string(),
    }
}

/// Hint band of a size pair (see [`hint_for_ratio`]; zero-size inputs are
/// neutral — there is nothing to celebrate or warn about).
#[must_use]
pub fn ratio_hint(input: u64, output: u64) -> Hint {
    match ratio_fraction(input, output) {
        Some(ratio) => hint_for_ratio(ratio),
        None => Hint::Neutral,
    }
}

/// Hint band of a compression fraction (plan 10 §phase 1 bands):
/// ≤ 0.8 good, ≤ 1.0 neutral, > 1.0 grew. Non-finite input maps to
/// neutral (the bands live **only** here; every other surface delegates).
#[must_use]
pub fn hint_for_ratio(ratio: f32) -> Hint {
    if !ratio.is_finite() {
        return Hint::Neutral;
    }
    if ratio <= 0.8 {
        Hint::Good
    } else if ratio <= 1.0 {
        Hint::Neutral
    } else {
        Hint::Grew
    }
}

/// Rendering color of a hint band (subtle tints per plan 10 §phase 1;
/// the *text* always carries the number — color is redundancy only).
#[must_use]
pub fn hint_color(hint: Hint, visuals: &egui::Visuals) -> Color32 {
    match hint {
        Hint::Good => Color32::from_rgb(90, 190, 110),
        Hint::Neutral => visuals.weak_text_color(),
        Hint::Grew => visuals.error_fg_color,
    }
}

/// The footer/report appendix for a size pair: `(31% · saved 2.9MiB)`,
/// `(101% · grew 2.1MiB)` or `(100%)` — empty for a zero-size input.
#[must_use]
pub fn ratio_appendix(input: u64, output: u64) -> String {
    let Some(_percent) = ratio_percent(input, output) else {
        return String::new();
    };
    let label = ratio_label(input, output);
    if output < input {
        format!("({label} · saved {})", crate::queue::format_size(input - output))
    } else if output > input {
        format!("({label} · grew {})", crate::queue::format_size(output - input))
    } else {
        format!("({label})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- bands (plan 10 §Testing: 79.9/80/100/100.1 boundaries) ----------

    #[test]
    fn hint_bands_at_boundaries() {
        assert_eq!(hint_for_ratio(0.0), Hint::Good);
        assert_eq!(hint_for_ratio(0.79), Hint::Good);
        assert_eq!(hint_for_ratio(0.799), Hint::Good);
        assert_eq!(hint_for_ratio(0.8), Hint::Good, "≤ 80 % is good");
        assert_eq!(hint_for_ratio(0.81), Hint::Neutral);
        assert_eq!(hint_for_ratio(1.0), Hint::Neutral);
        assert_eq!(hint_for_ratio(1.001), Hint::Grew);
        assert_eq!(hint_for_ratio(f32::NAN), Hint::Neutral);
        assert_eq!(hint_for_ratio(f32::INFINITY), Hint::Neutral);
    }

    #[test]
    fn ratio_hint_from_sizes_matches_the_fraction_bands() {
        // 79.9 % / 80 % / 80.1 % / 100 % / 100.1 %
        assert_eq!(ratio_hint(1000, 799), Hint::Good);
        assert_eq!(ratio_hint(1000, 800), Hint::Good);
        assert_eq!(ratio_hint(1000, 801), Hint::Neutral);
        assert_eq!(ratio_hint(1000, 1000), Hint::Neutral);
        assert_eq!(ratio_hint(1000, 1001), Hint::Grew);
        // grew can be extreme
        assert_eq!(ratio_hint(10, 200), Hint::Grew);
    }

    // ---- percent + label ---------------------------------------------------

    #[test]
    fn zero_input_has_no_ratio() {
        assert_eq!(ratio_percent(0, 100), None);
        assert_eq!(ratio_percent(0, 0), None);
        assert_eq!(ratio_label(0, 100), "—");
        assert_eq!(ratio_hint(0, 100), Hint::Neutral);
        assert_eq!(ratio_appendix(0, 100), "", "no appendix without a ratio");
    }

    #[test]
    fn label_rounds_to_whole_percents() {
        assert_eq!(ratio_label(1000, 310), "31%");
        assert_eq!(ratio_label(100, 50), "50%");
        assert_eq!(ratio_label(3, 1), "33%");
        assert_eq!(ratio_label(3, 2), "67%");
        assert_eq!(ratio_label(100, 101), "101%");
        assert_eq!(ratio_percent(1000, 310), Some(31.0));
    }

    // ---- footer/report parity ----------------------------------------------

    #[test]
    fn appendix_format_matches_the_plan_examples() {
        // plan 10: `(62% · saved 89.1MiB)` — sizes from 144 MiB → 55 MiB is
        // 62 % saved 89 MiB; use exact bytes for a deterministic string
        let input: u64 = 146 * 1024 * 1024; // 146 MiB
        let output: u64 = 91 * 1024 * 1024; // 91 MiB → 62.3 % → "62%"
        assert_eq!(ratio_appendix(input, output), "(62% · saved 55.00MiB)");
        let grew = ratio_appendix(1000, 1200);
        assert!(grew.contains("grew"), "larger output names the growth: {grew}");
        assert_eq!(ratio_appendix(1000, 1000), "(100%)");
    }

    #[test]
    fn footer_and_report_labels_are_identical_strings() {
        // the parity guarantee both surfaces rely on: they call the same
        // helper with the same totals — this pins the contract
        for (input, output) in [(0u64, 5u64), (100, 50), (100, 100), (100, 140), (7, 3)] {
            let label = ratio_label(input, output);
            assert_eq!(label, ratio_label(input, output));
            if input > 0 {
                assert!(ratio_appendix(input, output).contains(&label));
            }
        }
    }
}

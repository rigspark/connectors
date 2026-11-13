//! Rough YouTube earnings ranges from public creator-reported RPMs.
//!
//! RPM = what the creator keeps per 1,000 views (after YouTube's share), across all views
//! including unmonetized ones. The table below is a mid-point for long-form videos watched
//! mostly from the US/CA/GB/AU, assembled from widely published creator reports; it is
//! deliberately simple and transparent. Real RPMs vary 3–5× by audience, season and format.

use serde::Serialize;

/// (category, mid RPM in USD for tier-1 long-form).
const CATEGORY_RPM: &[(&str, f64)] = &[
    ("Finance", 12.0),
    ("Business", 10.0),
    ("Technology", 7.0),
    ("Education", 6.0),
    ("Autos & Vehicles", 6.0),
    ("Health", 6.0),
    ("Howto & Style", 4.5),
    ("Travel", 4.0),
    ("Food", 4.0),
    ("Lifestyle", 4.0),
    ("Sports", 3.5),
    ("News & Politics", 3.0),
    ("Pets & Animals", 3.0),
    ("Nonprofits & Activism", 3.0),
    ("People & Blogs", 2.5),
    ("Entertainment", 2.5),
    ("Film & Animation", 2.5),
    ("Gaming", 2.5),
    ("Comedy", 2.0),
    ("Religion", 2.0),
    ("Music", 1.5),
];

/// YouTube `categoryId` → category used above.
pub fn category_for_id(id: &str) -> &'static str {
    match id {
        "1" => "Film & Animation",
        "2" => "Autos & Vehicles",
        "10" => "Music",
        "15" => "Pets & Animals",
        "17" => "Sports",
        "19" => "Travel",
        "20" => "Gaming",
        "22" => "People & Blogs",
        "23" => "Comedy",
        "24" => "Entertainment",
        "25" => "News & Politics",
        "26" => "Howto & Style",
        "27" => "Education",
        "28" => "Technology",
        "29" => "Nonprofits & Activism",
        _ => "Entertainment",
    }
}

pub fn categories() -> Vec<&'static str> {
    CATEGORY_RPM.iter().map(|(c, _)| *c).collect()
}

/// Multiplier on tier-1 RPM by audience country.
pub fn country_factor(country: Option<&str>) -> (f64, &'static str) {
    let c = country.unwrap_or("").to_ascii_uppercase();
    match c.as_str() {
        "US" | "CA" | "GB" | "AU" | "NZ" | "CH" | "NO" => (1.0, "tier 1"),
        "DE" | "NL" | "SE" | "DK" | "FI" | "IE" | "AT" | "BE" | "FR" | "JP" | "SG" | "KR"
        | "AE" | "IL" | "LU" => (0.7, "tier 2"),
        "IT" | "ES" | "PT" | "PL" | "CZ" | "SA" | "QA" | "KW" | "HK" | "TW" | "GR" => {
            (0.45, "tier 3")
        }
        "BR" | "MX" | "AR" | "CL" | "CO" | "ZA" | "MY" | "TH" | "TR" | "RO" | "HU" | "UA" => {
            (0.25, "tier 4")
        }
        "IN" | "PK" | "BD" | "ID" | "PH" | "VN" | "EG" | "NG" | "KE" | "NP" | "LK" => {
            (0.12, "tier 5")
        }
        "" => (0.5, "unknown (assumed mixed audience)"),
        _ => (0.3, "other"),
    }
}

/// Shorts revenue-share RPM range (USD per 1,000 views), mid point.
const SHORTS_RPM_MID: f64 = 0.06;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Estimate {
    pub monthly_views: u64,
    pub category: String,
    pub audience: String,
    pub rpm_low: f64,
    pub rpm_mid: f64,
    pub rpm_high: f64,
    pub monthly_usd_low: f64,
    pub monthly_usd_mid: f64,
    pub monthly_usd_high: f64,
    pub notes: Vec<String>,
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `shorts_share`: 0 = all long-form, 1 = all Shorts.
pub fn estimate(
    monthly_views: u64,
    category: &str,
    country: Option<&str>,
    shorts_share: f64,
) -> Estimate {
    let cat = CATEGORY_RPM
        .iter()
        .find(|(c, _)| {
            c.eq_ignore_ascii_case(category)
                || c.to_lowercase().starts_with(&category.to_lowercase())
        })
        .copied()
        .unwrap_or(("Entertainment", 2.5));
    let (factor, tier) = country_factor(country);
    let share = shorts_share.clamp(0.0, 1.0);
    let long_mid = cat.1 * factor;
    let shorts_mid = SHORTS_RPM_MID * factor.sqrt(); // Shorts vary less by country
    let mid = long_mid * (1.0 - share) + shorts_mid * share;
    let (low, high) = (mid * 0.5, mid * 1.8);
    let k = monthly_views as f64 / 1000.0;
    let mut notes = vec![
        "Rough range from public creator reports, not a lookup of real earnings.".to_owned(),
        "Assumes the channel is in the YouTube Partner Program; non-monetized channels earn $0 from ads.".to_owned(),
    ];
    if share > 0.0 {
        notes.push(format!(
            "{:.0}% of views treated as Shorts (≈${SHORTS_RPM_MID:.2}/1k mid).",
            share * 100.0
        ));
    }
    Estimate {
        monthly_views,
        category: cat.0.into(),
        audience: format!("{} ({tier})", country.unwrap_or("unknown")),
        rpm_low: r2(low),
        rpm_mid: r2(mid),
        rpm_high: r2(high),
        monthly_usd_low: r2(low * k),
        monthly_usd_mid: r2(mid * k),
        monthly_usd_high: r2(high * k),
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_sensibly() {
        let fin_us = estimate(1_000_000, "finance", Some("US"), 0.0);
        let game_in = estimate(1_000_000, "Gaming", Some("IN"), 0.0);
        let shorts = estimate(1_000_000, "Finance", Some("US"), 1.0);
        assert!(fin_us.monthly_usd_mid > game_in.monthly_usd_mid * 10.0);
        assert!(shorts.monthly_usd_mid < fin_us.monthly_usd_mid / 50.0);
        assert_eq!(fin_us.category, "Finance");
        assert!(
            fin_us.monthly_usd_low < fin_us.monthly_usd_mid
                && fin_us.monthly_usd_mid < fin_us.monthly_usd_high
        );
        assert_eq!(category_for_id("28"), "Technology");
    }
}

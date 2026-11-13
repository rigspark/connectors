//! Removes email addresses and phone numbers from third-party text (comments) before it reaches
//! the model, as ViewsFix's strip-PII hook did. Conservative on phones so that view counts,
//! dates and timestamps survive: a run of digits counts as a phone number when it starts with
//! `+` and has 8–15 digits, or has 10–15 digits with at most spaces, dashes, dots and brackets
//! between them.

pub const EMAIL: &str = "[email removed]";
pub const PHONE: &str = "[phone removed]";

pub fn scrub_contacts(text: &str) -> String {
    scrub_phones(&scrub_emails(text))
}

fn scrub_emails(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let core_start = word
            .find(|c: char| c.is_alphanumeric())
            .unwrap_or(word.len());
        let core_end = word
            .rfind(|c: char| c.is_alphanumeric())
            .map_or(core_start, |e| e + 1);
        let core = &word[core_start..core_end.max(core_start)];
        if is_email(core) {
            out.push_str(&word[..core_start]);
            out.push_str(EMAIL);
            out.push_str(&word[core_end..]);
        } else {
            out.push_str(word);
        }
    }
    out
}

fn is_email(s: &str) -> bool {
    let Some((user, domain)) = s.split_once('@') else {
        return false;
    };
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    !user.is_empty()
        && user
            .chars()
            .all(|c| c.is_alphanumeric() || "._%+-".contains(c))
        && !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || ".-".contains(c))
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

fn scrub_phones(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i] == '+' || chars[i] == '(' || chars[i].is_ascii_digit();
        let after_word = i > 0 && chars[i - 1].is_alphanumeric();
        if starts && !after_word {
            // extend over digits and phone separators
            let mut j = i;
            let mut digits = 0;
            let mut last_digit = i;
            while j < chars.len() {
                let c = chars[j];
                if c.is_ascii_digit() {
                    digits += 1;
                    last_digit = j;
                } else if !(c == ' '
                    || c == '-'
                    || c == '.'
                    || c == '('
                    || c == ')'
                    || (c == '+' && j == i))
                {
                    break;
                }
                j += 1;
            }
            let end = last_digit + 1;
            let plus = chars[i] == '+';
            let next_is_word = end < chars.len() && chars[end].is_alphanumeric();
            let phone = !next_is_word
                && ((plus && (8..=15).contains(&digits)) || (10..=15).contains(&digits));
            if phone {
                out.push_str(PHONE);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_emails_keeping_punctuation() {
        assert_eq!(
            scrub_contacts("mail me at jo.doe+yt@gmail.com, thanks"),
            "mail me at [email removed], thanks"
        );
        assert_eq!(
            scrub_contacts("@handle and a@b are fine"),
            "@handle and a@b are fine"
        );
    }

    #[test]
    fn removes_phone_numbers() {
        assert_eq!(
            scrub_contacts("call +91 84110 66881 now"),
            "call [phone removed] now"
        );
        assert_eq!(
            scrub_contacts("US: (415) 555-0132 ok"),
            "US: [phone removed] ok"
        );
    }

    #[test]
    fn keeps_counts_dates_and_timestamps() {
        for s in [
            "1,000,000 views!",
            "posted 2026-11-12",
            "at 12:34 he says",
            "top 10 of 2025",
            "v2.3.1 release",
            "item 12345678",
        ] {
            assert_eq!(scrub_contacts(s), s);
        }
    }
}

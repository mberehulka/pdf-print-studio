//! Page-range parsing and the page selection pipeline.
//!
//! Range syntax (1-based):  `1-3, 5, 8-end, -4, 10-, 7-2 (descending), 3*2 (repeat)`.
//! Separators: comma, semicolon or whitespace.  `end`/`last`/`$` mean the last page.

use crate::settings::{RangeMode, Settings, Subset};
use std::collections::{BTreeSet, HashSet};

fn parse_num(s: &str, n: usize) -> Result<usize, String> {
    let s = s.trim();
    match s.to_ascii_lowercase().as_str() {
        "end" | "last" | "$" => return Ok(n),
        _ => {}
    }
    let v: usize = s.parse().map_err(|_| crate::trf!("'{}' is not a page number", s))?;
    if v == 0 || v > n {
        return Err(crate::trf!("page {} is out of range (1-{})", v, n));
    }
    Ok(v)
}

pub fn parse_ranges(input: &str, n: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    if n == 0 {
        return Ok(out);
    }
    let normalized = input.replace([';', ','], " ");
    for tok in normalized.split_whitespace() {
        let (body, repeat) = match tok.split_once(['*', 'x', 'X']) {
            Some((b, r)) => (
                b,
                r.parse::<usize>()
                    .map_err(|_| crate::trf!("bad repeat count in '{}'", tok))?
                    .clamp(1, 999),
            ),
            None => (tok, 1),
        };
        let pages: Vec<usize> = if let Some((a, b)) = body.split_once('-') {
            let a = if a.trim().is_empty() { 1 } else { parse_num(a, n)? };
            let b = if b.trim().is_empty() { n } else { parse_num(b, n)? };
            if a <= b {
                (a..=b).collect()
            } else {
                (b..=a).rev().collect()
            }
        } else {
            vec![parse_num(body, n)?]
        };
        for p in pages {
            for _ in 0..repeat {
                out.push(p - 1);
            }
        }
    }
    Ok(out)
}

/// Final ordered list of 0-based page indices to print (duplicates allowed).
pub fn compute_selection(
    s: &Settings,
    page_count: usize,
    current: usize,
    picked: &BTreeSet<usize>,
) -> Result<Vec<usize>, String> {
    let mut list: Vec<usize> = match s.range_mode {
        RangeMode::All => (0..page_count).collect(),
        RangeMode::Current => vec![current.min(page_count.saturating_sub(1))],
        RangeMode::Custom => {
            if s.custom_range.trim().is_empty() {
                (0..page_count).collect()
            } else {
                parse_ranges(&s.custom_range, page_count)?
            }
        }
        RangeMode::Picked => picked.iter().copied().filter(|&p| p < page_count).collect(),
    };

    if !s.exclude.trim().is_empty() {
        let ex: HashSet<usize> = parse_ranges(&s.exclude, page_count)
            .map_err(|e| crate::trf!("Exclude: {}", e))?
            .into_iter()
            .collect();
        list.retain(|p| !ex.contains(p));
    }

    match s.subset {
        Subset::All => {}
        Subset::Odd => list.retain(|p| (p + 1) % 2 == 1),
        Subset::Even => list.retain(|p| (p + 1) % 2 == 0),
    }

    let nth = s.every_nth.max(1) as usize;
    if nth > 1 {
        let off = s.nth_offset as usize % nth;
        list = list
            .into_iter()
            .enumerate()
            .filter(|(i, _)| i % nth == off)
            .map(|(_, p)| p)
            .collect();
    }

    if s.reverse {
        list.reverse();
    }
    Ok(list)
}

/// Compact human description of a set of pages: "1-3, 5, 9".
pub fn describe(pages: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        let mut j = i;
        while j + 1 < pages.len() && pages[j + 1] == pages[j] + 1 {
            j += 1;
        }
        if j > i {
            out.push(format!("{}-{}", start + 1, pages[j] + 1));
        } else {
            out.push(format!("{}", start + 1));
        }
        i = j + 1;
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_ranges("1-3, 5", 10).unwrap(), vec![0, 1, 2, 4]);
        assert_eq!(parse_ranges("8-", 10).unwrap(), vec![7, 8, 9]);
        assert_eq!(parse_ranges("3-1", 10).unwrap(), vec![2, 1, 0]);
        assert_eq!(parse_ranges("2*3 end", 10).unwrap(), vec![1, 1, 1, 9]);
        assert!(parse_ranges("11", 10).is_err());
        assert_eq!(describe(&[0, 1, 2, 4, 8]), "1-3, 5, 9");
    }
}

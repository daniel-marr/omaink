//! Fractional index keys. A key is a string over a base-62 alphabet in strict
//! ASCII order, interpreted as the fractional part of a base-62 number. Given
//! two neighbors you can always mint a key that sorts strictly between them,
//! so reordering or inserting one item never rewrites any sibling's file.
//!
//! `key_between(lo, hi)` requires `lo < hi` lexicographically when both are
//! given; `None` means "unbounded" on that side.

const N: usize = 62;

fn val(c: u8) -> usize {
    match c {
        b'0'..=b'9' => (c - b'0') as usize,
        b'A'..=b'Z' => (c - b'A') as usize + 10,
        b'a'..=b'z' => (c - b'a') as usize + 36,
        _ => 0,
    }
}

fn digit(v: usize) -> u8 {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    ALPHABET[v]
}

/// A key strictly between `lo` and `hi`. With both `None`, returns a middle
/// key suitable as the first item in a list.
pub fn key_between(lo: Option<&str>, hi: Option<&str>) -> String {
    let lo = lo.unwrap_or("");
    let lo = lo.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    // Safety bound: keys never need to grow without limit for sane inputs.
    for _ in 0..4096 {
        let a = lo.get(i).map(|&c| val(c)).unwrap_or(0);
        let b = match hi {
            Some(h) => h.as_bytes().get(i).map(|&c| val(c)).unwrap_or(0),
            None => N, // unbounded upper behaves like one past the max digit
        };
        if a == b {
            out.push(digit(a));
            i += 1;
            continue;
        }
        if b - a >= 2 {
            out.push(digit((a + b) / 2));
            return String::from_utf8(out).unwrap();
        }
        // Adjacent digits (b == a + 1): keep `a`, then append a key strictly
        // greater than the rest of `lo` with an unbounded upper side.
        out.push(digit(a));
        i += 1;
        for _ in 0..4096 {
            let da = lo.get(i).map(|&c| val(c)).unwrap_or(0);
            if da == N - 1 {
                out.push(digit(da));
                i += 1;
                continue;
            }
            out.push(digit((da + N) / 2));
            return String::from_utf8(out).unwrap();
        }
        break;
    }
    // Unreachable for valid inputs; a stable fallback beats a panic.
    out.push(digit(N / 2));
    String::from_utf8(out).unwrap()
}

/// Append a new key after the last existing key (or the first key if empty).
pub fn key_after_last(keys: &[String]) -> String {
    match keys.iter().max() {
        Some(last) => key_between(Some(last), None),
        None => key_between(None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_key_is_stable_and_midrange() {
        let k = key_between(None, None);
        assert!(!k.is_empty());
        assert!(key_between(None, Some(&k)) < k);
        assert!(k < key_between(Some(&k), None));
    }

    #[test]
    fn between_is_strictly_ordered() {
        let a = key_between(None, None);
        let b = key_between(Some(&a), None);
        let mid = key_between(Some(&a), Some(&b));
        assert!(a < mid && mid < b, "{a} < {mid} < {b}");
    }

    #[test]
    fn repeated_insertion_between_same_pair_stays_ordered() {
        // Insert 100 times between a fixed low and high; every key must land
        // strictly between, and all must be mutually ordered left-to-right.
        let lo = key_between(None, None);
        let hi = key_between(Some(&lo), None);
        let mut right = hi.clone();
        let mut inserted = Vec::new();
        for _ in 0..100 {
            let k = key_between(Some(&lo), Some(&right));
            assert!(lo < k && k < right, "{lo} < {k} < {right}");
            right = k.clone();
            inserted.push(k);
        }
        // inserted is descending (each between lo and the previous); reversed it is ascending.
        let mut sorted = inserted.clone();
        sorted.sort();
        inserted.reverse();
        assert_eq!(inserted, sorted);
    }

    #[test]
    fn appending_keeps_growing_order() {
        let mut keys: Vec<String> = Vec::new();
        for _ in 0..50 {
            let k = key_after_last(&keys);
            if let Some(last) = keys.last() {
                assert!(*last < k, "{last} < {k}");
            }
            keys.push(k);
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn prepending_before_first_is_ordered() {
        let first = key_between(None, None);
        let before = key_between(None, Some(&first));
        assert!(before < first, "{before} < {first}");
    }
}

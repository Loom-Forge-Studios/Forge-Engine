//! Keeping a virtualised list in step with a model: append what is new, relabel what
//! changed, and rebuild only when the order itself changed. A list of 10,000 history
//! entries therefore costs one row per new transaction, not 10,000.

use forge_ui::widgets::{RowItem, VirtualTree};

/// Bring `t`'s rows in line with `rows` (key, label). Returns how many rows it touched.
pub fn sync_rows(t: &mut VirtualTree, rows: &[(u64, String)]) -> usize {
    let n = t.row_count();
    let prefix_matches = n <= rows.len() && (0..n).all(|i| t.key_at(i) == Some(rows[i].0));
    if !prefix_matches {
        let keys: Vec<u64> = (0..n).filter_map(|i| t.key_at(i)).collect();
        for k in keys {
            t.remove(k);
        }
        t.extend(rows.iter().map(|(k, l)| (*k, RowItem::new(l.clone()))));
        return n + rows.len();
    }
    let mut touched = 0;
    for (k, l) in &rows[..n] {
        if t.label_of(*k) != Some(l.as_str()) {
            t.set_label(*k, l);
            touched += 1;
        }
    }
    if rows.len() > n {
        t.extend(rows[n..].iter().map(|(k, l)| (*k, RowItem::new(l.clone()))));
        touched += rows.len() - n;
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &[(u64, &str)]) -> Vec<(u64, String)> {
        v.iter().map(|(k, l)| (*k, l.to_string())).collect()
    }

    #[test]
    fn appends_and_relabels_without_rebuilding() {
        let mut t = VirtualTree::list("t");
        assert_eq!(sync_rows(&mut t, &rows(&[(1, "a"), (2, "b")])), 2);
        assert_eq!(
            sync_rows(&mut t, &rows(&[(1, "a"), (2, "b*"), (3, "c")])),
            2
        );
        assert_eq!(t.row_count(), 3);
        assert_eq!(t.label_of(2), Some("b*"));
        assert_eq!(
            sync_rows(&mut t, &rows(&[(1, "a"), (2, "b*"), (3, "c")])),
            0
        );
        // A removal in the middle rebuilds.
        assert_eq!(sync_rows(&mut t, &rows(&[(1, "a"), (3, "c")])), 5);
        assert_eq!(
            (t.key_at(0), t.key_at(1), t.row_count()),
            (Some(1), Some(3), 2)
        );
    }
}

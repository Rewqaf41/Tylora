//! Directional selection from actual window geometry, independent of BSP order.

use crate::{commands::Direction, registry::ManagedWindowId, Rect};

fn overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> f64 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

/// Prefers a candidate with overlap on the perpendicular axis, then the
/// nearest edge, then the smallest perpendicular center offset. ID breaks ties.
pub fn neighbor(
    current: ManagedWindowId,
    direction: Direction,
    windows: impl IntoIterator<Item = (ManagedWindowId, Rect)>,
) -> Option<ManagedWindowId> {
    let windows: Vec<_> = windows.into_iter().collect();
    let source = windows.iter().find(|(id, _)| *id == current)?.1;
    let (sx, sy) = (
        source.x + source.width / 2.0,
        source.y + source.height / 2.0,
    );
    windows
        .into_iter()
        .filter_map(|(id, rect)| {
            if id == current {
                return None;
            }
            let (cx, cy) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            let (forward, edge_distance, projection, offset) = match direction {
                Direction::Left => (
                    cx < sx,
                    (source.x - rect.x - rect.width).max(0.0),
                    overlap(
                        source.y,
                        source.y + source.height,
                        rect.y,
                        rect.y + rect.height,
                    ),
                    (cy - sy).abs(),
                ),
                Direction::Right => (
                    cx > sx,
                    (rect.x - source.x - source.width).max(0.0),
                    overlap(
                        source.y,
                        source.y + source.height,
                        rect.y,
                        rect.y + rect.height,
                    ),
                    (cy - sy).abs(),
                ),
                Direction::Up => (
                    cy < sy,
                    (source.y - rect.y - rect.height).max(0.0),
                    overlap(
                        source.x,
                        source.x + source.width,
                        rect.x,
                        rect.x + rect.width,
                    ),
                    (cx - sx).abs(),
                ),
                Direction::Down => (
                    cy > sy,
                    (rect.y - source.y - source.height).max(0.0),
                    overlap(
                        source.x,
                        source.x + source.width,
                        rect.x,
                        rect.x + rect.width,
                    ),
                    (cx - sx).abs(),
                ),
            };
            forward.then_some((id, projection == 0.0, edge_distance, offset))
        })
        .min_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| a.2.total_cmp(&b.2))
                .then_with(|| a.3.total_cmp(&b.3))
                .then_with(|| a.0.cmp(&b.0))
        })
        .map(|entry| entry.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProcessInstance;
    fn id(n: u64) -> ManagedWindowId {
        ManagedWindowId {
            process: ProcessInstance { pid: 1, epoch: 1 },
            token: n,
        }
    }
    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h).unwrap()
    }
    #[test]
    fn chooses_projected_neighbor_over_diagonal_and_is_stable() {
        let windows = vec![
            (id(1), rect(0.0, 0.0, 100.0, 100.0)),
            (id(2), rect(120.0, 120.0, 30.0, 30.0)),
            (id(3), rect(150.0, 20.0, 100.0, 50.0)),
        ];
        assert_eq!(
            neighbor(id(1), Direction::Right, windows.clone()),
            Some(id(3))
        );
        assert_eq!(neighbor(id(3), Direction::Left, windows), Some(id(1)));
    }
    #[test]
    fn returns_none_without_forward_candidate() {
        assert_eq!(
            neighbor(
                id(1),
                Direction::Left,
                [(id(1), rect(0.0, 0.0, 10.0, 10.0))]
            ),
            None
        );
    }
}

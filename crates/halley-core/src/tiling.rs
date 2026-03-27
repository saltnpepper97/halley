use crate::field::NodeId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn inset(&self, pad: f32) -> Rect {
        Rect {
            x: self.x + pad,
            y: self.y + pad,
            w: (self.w - 2.0 * pad).max(0.0),
            h: (self.h - 2.0 * pad).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileRole {
    Master,
    Stack,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: NodeId,
    pub role: TileRole,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MasterStackLayout {
    pub tiles: Vec<Tile>,
}

pub fn layout_master_stack(container: Rect, members: &[NodeId]) -> MasterStackLayout {
    if members.is_empty() {
        return MasterStackLayout { tiles: Vec::new() };
    }

    if members.len() == 1 {
        return MasterStackLayout {
            tiles: vec![Tile {
                id: members[0],
                role: TileRole::Master,
                rect: container,
            }],
        };
    }

    let master_w = (container.w * 0.6).clamp(0.0, container.w);
    let stack_w = (container.w - master_w).max(0.0);
    let master_rect = Rect {
        x: container.x,
        y: container.y,
        w: master_w,
        h: container.h,
    };
    let stack_rect = Rect {
        x: container.x + master_w,
        y: container.y,
        w: stack_w,
        h: container.h,
    };

    let mut tiles = Vec::with_capacity(members.len());
    tiles.push(Tile {
        id: members[0],
        role: TileRole::Master,
        rect: master_rect,
    });

    let stack_members = &members[1..];
    let stack_len = stack_members.len() as f32;
    let mut next_y = stack_rect.y;

    for (index, member) in stack_members.iter().enumerate().rev() {
        let height = if index == 0 {
            stack_rect.bottom() - next_y
        } else {
            stack_rect.h / stack_len
        }
        .max(0.0);

        tiles.push(Tile {
            id: *member,
            role: TileRole::Stack,
            rect: Rect {
                x: stack_rect.x,
                y: next_y,
                w: stack_rect.w,
                h: height,
            },
        });
        next_y += height;
    }

    tiles.sort_by_key(|tile| {
        members
            .iter()
            .position(|member| *member == tile.id)
            .unwrap_or(usize::MAX)
    });

    MasterStackLayout { tiles }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<NodeId> {
        (0..n).map(NodeId::new).collect()
    }

    #[test]
    fn empty_members_produces_no_tiles() {
        let layout = layout_master_stack(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &[],
        );

        assert!(layout.tiles.is_empty());
    }

    #[test]
    fn single_member_fills_container_as_master() {
        let members = ids(1);
        let container = Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 600.0,
        };

        let layout = layout_master_stack(container, &members);

        assert_eq!(layout.tiles.len(), 1);
        assert_eq!(layout.tiles[0].id, members[0]);
        assert_eq!(layout.tiles[0].role, TileRole::Master);
        assert_eq!(layout.tiles[0].rect, container);
    }

    #[test]
    fn master_stays_on_left_and_stack_on_right() {
        let members = ids(3);
        let layout = layout_master_stack(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &members,
        );

        assert_eq!(layout.tiles[0].role, TileRole::Master);
        assert_eq!(layout.tiles[0].id, members[0]);
        assert!(layout.tiles[0].rect.x < layout.tiles[1].rect.x);
        assert!(layout.tiles[0].rect.x < layout.tiles[2].rect.x);
    }

    #[test]
    fn stack_members_are_laid_out_bottom_up() {
        let members = ids(4);
        let layout = layout_master_stack(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 600.0,
            },
            &members,
        );

        let second = &layout.tiles[1];
        let third = &layout.tiles[2];
        let fourth = &layout.tiles[3];

        assert_eq!(second.id, members[1]);
        assert_eq!(third.id, members[2]);
        assert_eq!(fourth.id, members[3]);
        assert!(second.rect.y > third.rect.y);
        assert!(third.rect.y > fourth.rect.y);
        assert_eq!(fourth.rect.y, 0.0);
    }

    #[test]
    fn every_member_gets_geometry_without_overflow() {
        let members = ids(7);
        let layout = layout_master_stack(
            Rect {
                x: 50.0,
                y: 20.0,
                w: 1400.0,
                h: 900.0,
            },
            &members,
        );

        assert_eq!(layout.tiles.len(), members.len());
        assert!(
            layout
                .tiles
                .iter()
                .all(|tile| tile.rect.w >= 0.0 && tile.rect.h >= 0.0)
        );
        assert!(layout.tiles.iter().all(|tile| members.contains(&tile.id)));
    }
}

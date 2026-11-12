use ratatui::layout::Rect;

pub const COMPOSER_HEIGHT: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutAreas {
    pub navigator: Rect,
    pub transcript: Rect,
    pub blackboard: Option<Rect>,
    pub composer: Rect,
}

pub fn compute_layout(area: Rect, blackboard_collapsed: bool) -> LayoutAreas {
    let composer_height = COMPOSER_HEIGHT.min(area.height);
    let top_height = area.height.saturating_sub(composer_height);
    let composer_y = area.y + top_height;

    let composer = Rect {
        x: area.x,
        y: composer_y,
        width: area.width,
        height: composer_height,
    };

    let nav_width = ((area.width as u32 * 22) / 100) as u16;

    if blackboard_collapsed {
        let transcript_width = area.width.saturating_sub(nav_width);

        let navigator = Rect {
            x: area.x,
            y: area.y,
            width: nav_width,
            height: top_height,
        };

        let transcript = Rect {
            x: area.x + nav_width,
            y: area.y,
            width: transcript_width,
            height: top_height,
        };

        LayoutAreas {
            navigator,
            transcript,
            blackboard: None,
            composer,
        }
    } else {
        let blackboard_width = ((area.width as u32 * 22) / 100) as u16;
        let transcript_width = area.width.saturating_sub(nav_width + blackboard_width);

        let navigator = Rect {
            x: area.x,
            y: area.y,
            width: nav_width,
            height: top_height,
        };

        let transcript = Rect {
            x: area.x + nav_width,
            y: area.y,
            width: transcript_width,
            height: top_height,
        };

        let blackboard = Rect {
            x: area.x + nav_width + transcript_width,
            y: area.y,
            width: blackboard_width,
            height: top_height,
        };

        LayoutAreas {
            navigator,
            transcript,
            blackboard: Some(blackboard),
            composer,
        }
    }
}

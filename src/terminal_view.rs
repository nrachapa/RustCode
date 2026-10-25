//! The terminal canvas: renders the alacritty grid as iced [`canvas`] geometry.

use crate::app::Message;
use iced::mouse::{Cursor, ScrollDelta};
use iced::widget::canvas::{self, Action, Event, Frame, Geometry, Program, Text};
use iced::{Color, Font, Pixels, Point, Rectangle, Renderer, Size, Theme};
use iced_core::alignment::Vertical;
use iced_core::text::{Alignment, LineHeight, Shaping};
use rustcode::terminal::Terminal;

/// Cell metrics for an 11pt Menlo (width 0.6em, line 16px at 1x).
pub const CELL_W: f32 = 6.6;
pub const CELL_H: f32 = 16.0;
pub const FONT_SIZE: f32 = 11.0;

/// Draws one terminal. `focused` toggles the solid block cursor.
pub struct TerminalView<'a> {
    pub term: &'a Terminal,
    pub focused: bool,
}

/// The canvas widget for the terminal.
pub fn view<'a>(term: &'a Terminal, focused: bool) -> canvas::Canvas<TerminalView<'a>, Message> {
    canvas::Canvas::new(TerminalView { term, focused })
        .width(80.0 * CELL_W)
        .height(30.0 * CELL_H)
}

/// The background of the grid (matches `DEFAULT_BG_COLOR` in `terminal.rs`).
const BG: Color = Color::from_rgb8(15, 16, 20);
const CURSOR: Color = Color::from_rgb8(126, 126, 128);

impl Program<Message> for TerminalView<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        _bounds: Rectangle,
        _cursor: Cursor,
    ) -> Option<Action<Message>> {
        match event {
            Event::Mouse(iced::mouse::Event::ButtonPressed(_)) => {
                Some(Action::publish(Message::TermFocus))
            }
            Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => {
                // Wheel-up is positive y in winit; positive alacritty Delta
                // is *down*, hence the sign flip.
                let lines = match delta {
                    ScrollDelta::Lines { y, .. } => *y,
                    ScrollDelta::Pixels { y, .. } => *y / CELL_H,
                };
                Some(Action::publish(Message::TermScroll(-lines)))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), BG);

        let font = Font::with_name("Menlo");
        for line in self.term.lines() {
            for run in &line.runs {
                let (col, row) = (run.col as f32, line.row as f32);
                let char_count = run.text.chars().count().max(1);
                let text = Text {
                    content: run.text.clone(),
                    position: Point::new(col * CELL_W, row * CELL_H),
                    max_width: char_count as f32 * CELL_W,
                    color: Color::from_rgb8(run.fg.r, run.fg.g, run.fg.b),
                    size: Pixels(FONT_SIZE),
                    line_height: LineHeight::Relative(1.0),
                    font,
                    align_x: Alignment::Left,
                    align_y: Vertical::Top,
                    shaping: Shaping::Basic,
                };
                frame.fill_text(text);
            }
        }

        // Solid block cursor while focused.
        if self.focused
            && let Some((row, col)) = self.term.cursor()
        {
            let pos = Point::new(col as f32 * CELL_W, row as f32 * CELL_H);
            let size = Size::new(CELL_W, CELL_H);
            frame.fill_rectangle(pos, size, CURSOR);
        }

        vec![frame.into_geometry()]
    }
}

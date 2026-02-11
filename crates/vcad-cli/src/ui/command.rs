//! Command input widget.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::App;

/// Draw the command input.
pub fn draw_command(f: &mut Frame, area: Rect, app: &App) {
    let in_command_mode = app.command_mode();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if in_command_mode {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .title(if in_command_mode {
            " Command "
        } else {
            " : for command mode "
        });

    let inner = block.inner(area);
    f.render_widget(block, area);

    if in_command_mode {
        let input_line = Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Yellow)),
            Span::styled(
                &app.command_input,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]);

        let paragraph = Paragraph::new(input_line);
        f.render_widget(paragraph, inner);
    } else {
        let hint = Paragraph::new("Press ':' to enter command mode")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(hint, inner);
    }
}

//! Interactive Terminal UI (TUI) for browsing, searching, and managing observations.

use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use memlayer_client::MemlayerClient;
use memlayer_proto::{
    DeleteObservationRequest, ListObservationsRequest, Observation, SearchObservationsRequest,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::io;
use tonic::transport::Channel;

#[derive(PartialEq, Eq)]
enum InputMode {
    Normal,
    Search,
}

pub async fn run(
    client: &mut MemlayerClient<Channel>,
    project_name: &str,
    initial_query: Option<String>,
) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = main_loop(&mut terminal, client, project_name, initial_query).await;

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn fetch_observations(
    client: &mut MemlayerClient<Channel>,
    project_name: &str,
    query: &str,
) -> Vec<Observation> {
    if query.trim().is_empty() {
        let req = ListObservationsRequest {
            project_name: project_name.to_string(),
            limit: 100,
            ..Default::default()
        };
        if let Ok(resp) = client.list_observations(req).await {
            return resp.into_inner().observations;
        }
    } else {
        let req = SearchObservationsRequest {
            project_name: project_name.to_string(),
            query: query.to_string(),
            limit: 100,
            ..Default::default()
        };
        if let Ok(resp) = client.search_observations(req).await {
            return resp.into_inner().observations;
        }
    }
    Vec::new()
}

async fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &mut MemlayerClient<Channel>,
    project_name: &str,
    initial_query: Option<String>,
) -> anyhow::Result<()> {
    let mut search_query = initial_query.unwrap_or_default();
    let mut input_mode = InputMode::Normal;
    let mut list_state = ListState::default();
    let mut observations = fetch_observations(client, project_name, &search_query).await;

    if !observations.is_empty() {
        list_state.select(Some(0));
    }

    let mut status_msg = String::from("Press 'q' to quit, '/' to search, 'd' to delete");

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header & Search
                    Constraint::Min(10),  // Main split (List + Detail)
                    Constraint::Length(1), // Footer status bar
                ])
                .split(f.area());

            // 1. Search Bar
            let search_style = match input_mode {
                InputMode::Normal => Style::default().fg(Color::Gray),
                InputMode::Search => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            };
            let search_bar = Paragraph::new(format!(" Search: {}", search_query))
                .style(search_style)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" memlayer observation browser — project: [{}] ", project_name)),
                );
            f.render_widget(search_bar, chunks[0]);

            // 2. Main Content Split
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(chunks[1]);

            // Observations List
            let items: Vec<ListItem> = observations
                .iter()
                .map(|obs| {
                    let title = if obs.title.is_empty() {
                        obs.topic_key.as_deref().unwrap_or("-")
                    } else {
                        obs.title.as_str()
                    };
                    let line = format!("#{} [{}] {}", obs.id, obs.r#type, title);
                    ListItem::new(line)
                })
                .collect();

            let list_widget = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Observations "))
                .highlight_style(
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");
            f.render_stateful_widget(list_widget, main_chunks[0], &mut list_state);

            // Observation Detail View
            let selected_obs = list_state
                .selected()
                .and_then(|idx| observations.get(idx));

            let detail_text = match selected_obs {
                Some(obs) => vec![
                    Line::from(vec![
                        Span::styled("ID: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(obs.id.to_string()),
                        Span::raw(" | "),
                        Span::styled("Type: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(&obs.r#type),
                        Span::raw(" | "),
                        Span::styled("Scope: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(&obs.scope),
                    ]),
                    Line::from(vec![
                        Span::styled("Title: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(&obs.title),
                    ]),
                    Line::from(vec![
                        Span::styled("Topic Key: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(obs.topic_key.as_deref().unwrap_or("-")),
                    ]),
                    Line::from(vec![
                        Span::styled("Created At: ", Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw(&obs.created_at),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled("Content:", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))),
                    Line::from(obs.content.as_str()),
                ],
                None => vec![Line::from("No observation selected")],
            };

            let detail_widget = Paragraph::new(detail_text)
                .block(Block::default().borders(Borders::ALL).title(" Detail View "))
                .wrap(Wrap { trim: false });
            f.render_widget(detail_widget, main_chunks[1]);

            // 3. Status Bar
            let status_bar = Paragraph::new(Span::styled(
                format!(" {}", status_msg),
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ));
            f.render_widget(status_bar, chunks[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('/') | KeyCode::Tab => {
                            input_mode = InputMode::Search;
                            status_msg = String::from("SEARCH MODE: Type query, Press Enter or Esc to finish");
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if !observations.is_empty() {
                                let i = match list_state.selected() {
                                    Some(i) => (i + 1).min(observations.len() - 1),
                                    None => 0,
                                };
                                list_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if !observations.is_empty() {
                                let i = match list_state.selected() {
                                    Some(i) => i.saturating_sub(1),
                                    None => 0,
                                };
                                list_state.select(Some(i));
                            }
                        }
                        KeyCode::Char('d') => {
                            if let Some(idx) = list_state.selected() {
                                if let Some(obs) = observations.get(idx) {
                                    let req = DeleteObservationRequest {
                                        project_name: project_name.to_string(),
                                        key: Some(memlayer_proto::delete_observation_request::Key::Id(obs.id)),
                                        hard: false,
                                    };
                                    if client.delete_observation(req).await.is_ok() {
                                        status_msg = format!("Soft deleted observation #{}", obs.id);
                                        observations = fetch_observations(client, project_name, &search_query).await;
                                        if idx >= observations.len() && !observations.is_empty() {
                                            list_state.select(Some(observations.len() - 1));
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    },
                    InputMode::Search => match key.code {
                        KeyCode::Enter | KeyCode::Esc => {
                            input_mode = InputMode::Normal;
                            status_msg = String::from("Press 'q' to quit, '/' to search, 'd' to delete");
                        }
                        KeyCode::Backspace => {
                            search_query.pop();
                            observations = fetch_observations(client, project_name, &search_query).await;
                            list_state.select(if observations.is_empty() { None } else { Some(0) });
                        }
                        KeyCode::Char(c) => {
                            search_query.push(c);
                            observations = fetch_observations(client, project_name, &search_query).await;
                            list_state.select(if observations.is_empty() { None } else { Some(0) });
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    Ok(())
}

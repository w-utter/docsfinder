use color_eyre::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    layout::{Constraint, Flex, Layout, Rect},
    style::Stylize,
    text::Text,
    widgets::{Block, Cell, Clear, HighlightSpacing, Paragraph, Row, Wrap},
};

use std::collections::HashSet;

use crate::crates_api::Info;

use std::io::Stdout;

pub struct Tui {
    app: App,
    terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
}

impl Tui {
    pub fn setup(
        info_rx: crate::Receiver,
        send_tx: tokio_mpsc::UnboundedSender<()>,
        max_screen_entries: usize,
    ) -> color_eyre::Result<Self> {
        color_eyre::install()?;
        let terminal = ratatui::init();
        Ok(Self {
            app: App::new(info_rx, send_tx, max_screen_entries),
            terminal,
        })
    }

    pub fn run(self) -> color_eyre::Result<()> {
        let Self { terminal, app } = self;

        let result = app.run(terminal);

        ratatui::restore();
        result
    }
}

use tokio::sync::mpsc as tokio_mpsc;

struct SendRecvState {
    info_rx: crate::Receiver,
    send_tx: tokio_mpsc::UnboundedSender<()>,
    received_count: usize,
    inflight_count: usize,
}

impl SendRecvState {
    fn new(info_rx: crate::Receiver, send_tx: tokio_mpsc::UnboundedSender<()>) -> Self {
        Self {
            info_rx,
            send_tx,
            received_count: 0,
            inflight_count: 0,
        }
    }

    pub fn update(
        &mut self,
        max_screen_entries: usize,
        seen_crates: &mut HashSet<String>,
        filters: &Filters,
    ) -> Result<Info, bool> {
        if self.received_count >= max_screen_entries {
            self.inflight_count = 0;
            return Err(false);
        }

        let to_send = max_screen_entries
            .checked_sub(self.received_count + self.inflight_count)
            .unwrap_or_default();

        for _ in 0..to_send {
            let _ = self.send_tx.send(());
        }

        self.inflight_count += to_send;

        let info = self.info_rx.recv().ok_or(false)?;

        self.inflight_count = self.inflight_count.checked_sub(1).unwrap_or_default();

        let info = match info {
            Ok(info) => info,
            Err(_e) => {
                //FIXME: log info err
                return Err(true);
            }
        };

        if seen_crates.get(&info.name).is_some() {
            // FIXME: log duplicate?
            return Err(true);
        }
        seen_crates.insert(info.name.clone());

        if let Some(_reason) = filters.filter(&info) {
            // FIXME: log skipped
            return Err(true);
        }
        Ok(info)
    }

    fn set_received_count(&mut self, received: usize) {
        self.received_count = received;
    }

    fn update_iter<'a>(&'a mut self,
        max_screen_entries: usize,
        seen_crates: &'a mut HashSet<String>,
        filters: &'a Filters,
                   ) -> UpdateIter<'a> {
        UpdateIter {
            state: self,
            max_screen_entries,
            seen_crates,
            filters,
            has_update: false,
        }
    }
}

struct UpdateIter<'a> {
    state: &'a mut SendRecvState,
    seen_crates: &'a mut HashSet<String>,
    filters: &'a Filters,
    max_screen_entries: usize,
    has_update: bool,
}

impl <'a> Iterator for UpdateIter<'a> {
    type Item = Info;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.state.update(self.max_screen_entries, &mut self.seen_crates, &self.filters) {
                Ok(info) => {
                    self.has_update = true;
                    return Some(info);
                }
                Err(true) => self.has_update = true,
                Err(false) => return None,
            }
        }
    }
}

struct Entries {
    inner: Vec<Result<Info, Option<Info>>>,
    longest_name_len: usize,
    longest_description_len: usize,
    longest_keyword_len: usize,
    longest_category_len: usize,
}

impl Entries {
    fn new(initial_count: usize) -> Self {
        Self {
            inner: vec![Err(None); initial_count],
            longest_name_len: 0,
            longest_description_len: 0,
            longest_keyword_len: 0,
            longest_category_len: 0,
        }
    }

    fn insert_info(&mut self, item: Info) -> Option<Info> {
        if let Some(r) = self.inner.iter_mut().find(|res| res.is_err()) {
            *r = Ok(item);
            self.update_constraints();
            None
        } else {
            Some(item)
        }
    }

    fn info_iter(&self) -> impl Iterator<Item = &Info> {
        self.inner.iter().filter_map(|res| match res {
            Ok(r) => Some(r),
            Err(r) => r.as_ref(),
        })
    }

    fn update_constraints(&mut self) {
        self.longest_name_len = core::cmp::max(
            "name".len(),
            self.info_iter()
                .map(|info| info.name.len())
                .max()
                .unwrap_or_default(),
        );
        self.longest_description_len = core::cmp::max(
            "description".len(),
            self.info_iter()
                .filter_map(|info| info.description.as_ref().map(|desc| desc.len()))
                .max()
                .unwrap_or_default(),
        );
        self.longest_keyword_len = core::cmp::max(
            "keywords".len(),
            self.info_iter()
                .filter_map(|info| info.keywords.iter().map(|kw| kw.len()).max())
                .max()
                .unwrap_or_default(),
        );
        self.longest_category_len = core::cmp::max(
            "categories".len(),
            self.info_iter()
                .filter_map(|info| info.categories.iter().map(|kw| kw.len()).max())
                .max()
                .unwrap_or_default(),
        );
    }

    fn entries_len(&self) -> usize {
        self.inner.iter().filter(|r| r.is_ok()).count()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn iter(&self) -> impl Iterator<Item = &Result<Info, Option<Info>>> {
        self.inner.iter()
    }

    fn next_entry_idx(&self, cur: Option<usize>) -> Option<usize> {
        let offset = match cur {
            Some(cur) if cur >= self.len() - 1 => {
                return self.inner.iter().position(|res| res.is_ok());
            }
            Some(cur) => cur + 1,
            None => 0,
        };

        let slice = &self.inner[offset..];

        if let Some(pos) = slice.iter().position(|res| res.is_ok()) {
            return Some(offset + pos);
        }

        self.inner.iter().position(|res| res.is_ok())
    }

    fn prev_entry_idx(&self, cur: Option<usize>) -> Option<usize> {
        match cur {
            Some(0) | None => self.inner.iter().rposition(|res| res.is_ok()),
            Some(cur) => {
                let slice = &self.inner[..cur];
                if let Some(pos) = slice.iter().rposition(|res| res.is_ok()) {
                    return Some(pos);
                }
                self.inner.iter().rposition(|res| res.is_ok())
            }
        }
    }

    fn mark_for_replacement(
        &mut self,
        idx: usize,
        send_ctx: &mut SendRecvState,
    ) {
        match self.inner.get_mut(idx) {
            None | Some(Err(_)) => (),
            Some(res) => {
                use chrono::{DateTime, Utc};
                const DUMMY: Info = Info {
                    keywords: vec![],
                    categories: vec![],
                    last_update: DateTime::<Utc>::MIN_UTC,
                    created_at: DateTime::<Utc>::MIN_UTC,
                    description: None,
                    version: String::new(),
                    stable_version: None,
                    downloads: 0,
                    recent_downloads: None,
                    name: String::new(),
                    repo_link: None,
                    docs_link: None,
                };

                *res = match res {
                    Ok(r) => Err(Some(core::mem::replace(r, DUMMY))),
                    _ => unreachable!(),
                };
                send_ctx.set_received_count(self.entries_len());
            }
        }
    }

    fn clear_all(&mut self, send_ctx: &mut SendRecvState) {
        for entry in self.inner.iter_mut() {
            if entry.is_err() {
                continue;
            }

            use chrono::{DateTime, Utc};
            const DUMMY: Info = Info {
                keywords: vec![],
                categories: vec![],
                last_update: DateTime::<Utc>::MIN_UTC,
                created_at: DateTime::<Utc>::MIN_UTC,
                description: None,
                version: String::new(),
                stable_version: None,
                downloads: 0,
                recent_downloads: None,
                name: String::new(),
                repo_link: None,
                docs_link: None,
            };

            *entry = match entry {
                Ok(r) => Err(Some(core::mem::replace(r, DUMMY))),
                _ => unreachable!(),
            };
        }
        send_ctx.set_received_count(self.entries_len());
    }

    fn is_first_entry(&self, idx: usize) -> bool {
        let slice = &self.inner[..idx];
        !slice.iter().any(|res| res.is_ok())
    }

    fn get(&self, idx: usize) -> Option<&Info> {
        match self.inner.get(idx)? {
            Ok(i) => Some(i),
            _ => None,
        }
    }

    fn resize(&mut self, count: usize) {
        if count > self.len() {
            let diff = count - self.len();
            self.inner.extend(vec![Err(None); diff]);
        } else {
            self.inner.sort_by(|a, b| {
                use std::cmp::Ordering;
                match (a, b) {
                    (Ok(_), Ok(_)) | (Err(Some(_)), Err(Some(_))) | (Err(None), Err(None)) => Ordering::Equal,
                    (Ok(_), _) => Ordering::Less,
                    (_, Ok(_)) => Ordering::Greater,
                    (Err(Some(_)), _) => Ordering::Less,
                    (_, Err(Some(_))) => Ordering::Less,
                }
            });
            self.inner.truncate(count);
        }
    }
}

use ratatui::widgets::{Table, TableState};

pub struct App {
    send_recv_state: SendRecvState,
    max_screen_entries: usize,
    entries: Entries,
    table_state: TableState,
    colors: TableColors,
    seen_crates: HashSet<String>,
    filters: Filters,
    prev_max_info_width: usize,
    menu_state: MenuState,
    hide_footer: bool,
}

#[derive(Clone, Copy)]
enum CurrentMenu {
    Filters(SelectedFilter),
    FullscreenInfo,
}

#[derive(Clone, Copy)]
enum SelectedFilter {
    Keywords,
    Duration,
}

impl SelectedFilter {
    fn switch_filter(&self, this: &mut App) {
        let switched = match self {
            Self::Keywords => Self::Duration,
            Self::Duration => Self::Keywords,
        };
        let Some((CurrentMenu::Filters(f), _)) = &mut this.menu_state.current else {
            return;
        };
        *f = switched;
    }
}

impl CurrentMenu {
    fn handle_input_char(&self, this: &mut App, char: char) {
        match self {
            Self::Filters(f) => match f {
                SelectedFilter::Keywords => this.filters.keywords_text.push(char),
                SelectedFilter::Duration => this.filters.duration_text.push(char),
            },
            Self::FullscreenInfo => (),
        }
    }

    fn handle_input_enter(&self, this: &mut App) {
        match self {
            Self::Filters(f) => match f {
                SelectedFilter::Keywords => {
                    this.filters.keywords = this
                        .filters
                        .keywords_text
                        .split([' ', ','])
                        .filter(|w| !w.is_empty())
                        .map(|w| w.to_lowercase())
                        .collect::<HashSet<_>>()
                }
                SelectedFilter::Duration => {
                    if let Ok(dur) = this.filters.duration_text.parse::<humantime::Duration>() {
                        this.filters.max_update = Some(*dur);
                    }
                }
            },
            Self::FullscreenInfo => (),
        }
    }

    fn handle_input_backspace(&self, this: &mut App) {
        match self {
            Self::Filters(f) => match f {
                SelectedFilter::Keywords => {
                    let _ = this.filters.keywords_text.pop();
                }
                SelectedFilter::Duration => {
                    let _ = this.filters.duration_text.pop();
                }
            },
            Self::FullscreenInfo => (),
        }
    }

    fn popup_title(&self) -> &str {
        match self {
            Self::Filters(_) => "Filters",
            Self::FullscreenInfo => "Crate Info",
        }
    }

    fn render_menu(
        &self,
        input_mode: &InputMode,
        area: Rect,
        now: &chrono::DateTime<chrono::Utc>,
        frame: &mut Frame,
        this: &App,
    ) {
        match self {
            Self::Filters(kind) => {
                let vertical = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Length(1),
                    Constraint::Length(3),
                    Constraint::Fill(1),
                ]);
                let [
                    keywords_area,
                    keywords_input_area,
                    duration_area,
                    duration_input_area,
                    _,
                ] = vertical.areas(area);

                let help_message = Paragraph::new("filter keywords, separated by commas");
                frame.render_widget(help_message, keywords_area);

                let input = Paragraph::new(this.filters.keywords_text.as_str())
                    .style(match (&input_mode, kind) {
                        (InputMode::Normal, SelectedFilter::Keywords) => Style::default()
                            .add_modifier(Modifier::REVERSED)
                            .fg(this.colors.selected_cell_style_fg),
                        (InputMode::Editing, SelectedFilter::Keywords) => {
                            Style::default().fg(this.colors.selected_row_style_fg)
                        }
                        _ => Style::default(),
                    })
                    .block(Block::bordered().title("Keywords"));

                frame.render_widget(input, keywords_input_area);

                let help_message = Paragraph::new("Maximum allowed duration");
                frame.render_widget(help_message, duration_area);

                let input = Paragraph::new(this.filters.duration_text.as_str())
                    .style(match (&input_mode, kind) {
                        (InputMode::Normal, SelectedFilter::Duration) => Style::default()
                            .add_modifier(Modifier::REVERSED)
                            .fg(this.colors.selected_cell_style_fg),
                        (InputMode::Editing, SelectedFilter::Duration) => {
                            Style::default().fg(this.colors.selected_row_style_fg)
                        }
                        _ => Style::default(),
                    })
                    .block(Block::bordered().title("Duration"));

                frame.render_widget(input, duration_input_area);
            }
            Self::FullscreenInfo => {
                if let Some(Some(info)) = this.table_state.selected().map(|s| this.entries.get(s)) {
                    info.render_full(now, area, frame)
                }
            }
        }
    }

    fn move_up_input(&self, app: &mut App) {
        match self {
            Self::Filters(f) => f.switch_filter(app),
            Self::FullscreenInfo => (),
        }
    }

    fn move_down_input(&self, app: &mut App) {
        match self {
            Self::Filters(f) => f.switch_filter(app),
            Self::FullscreenInfo => (),
        }
    }
}

enum InputMode {
    Normal,
    Editing,
}

#[derive(Default)]
struct MenuState {
    current: Option<(CurrentMenu, InputMode)>,
}

#[derive(Default)]
struct Filters {
    max_update: Option<std::time::Duration>,
    keywords: HashSet<String>,
    keywords_text: String,
    duration_text: String,
}

pub enum FilterReason<'a> {
    Outdated,
    Keyword(&'a str),
}

impl Filters {
    fn filter<'a>(&self, info: &'a Info) -> Option<FilterReason<'a>> {
        if let Some(max_dur) = self.max_update.as_ref() {
            let now = chrono::Utc::now();

            if let Ok(last_upd) = now.signed_duration_since(&info.last_update).to_std() {
                if last_upd > *max_dur {
                    return Some(FilterReason::Outdated);
                }
            }
        }

        {
            for word in info.name.split(['-', '_']).filter(|w| !w.is_empty()) {
                if self.keywords.contains(&word.to_lowercase()) {
                    return Some(FilterReason::Keyword(word));
                }
            }

            if let Some(desc) = info.description.as_ref() {
                for word in desc.split_whitespace() {
                    for span in word.split(['-', '_']).filter(|w| !w.is_empty()) {
                        if self.keywords.contains(&span.to_lowercase()) {
                            return Some(FilterReason::Keyword(span));
                        }
                    }

                    if self.keywords.contains(&word.to_lowercase()) {
                        return Some(FilterReason::Keyword(word));
                    }
                }
            }

            for kw in info.keywords.iter() {
                if self.keywords.contains(&kw.to_lowercase()) {
                    return Some(FilterReason::Keyword(kw));
                }
            }

            for cat in info.categories.iter() {
                for sub in cat.split("::").filter(|s| !s.is_empty()) {
                    if self.keywords.contains(&sub.to_lowercase()) {
                        return Some(FilterReason::Keyword(sub));
                    }
                }
                if self.keywords.contains(&cat.to_lowercase()) {
                    return Some(FilterReason::Keyword(cat));
                }
            }
        }
        None
    }
}

#[test]
fn keyword_filters() {
    let mut filters = Filters::default();
    filters.keywords.insert("azure".to_string());

    use chrono::{DateTime, Utc};
    let dummy_1 = Info {
        keywords: vec![],
        categories: vec![],
        last_update: DateTime::<Utc>::MIN_UTC,
        created_at: DateTime::<Utc>::MIN_UTC,
        description: None,
        version: String::new(),
        stable_version: None,
        downloads: 0,
        recent_downloads: None,
        name: "azure_test_1".to_owned(),
        repo_link: None,
        docs_link: None,
    };

    assert!(filters.filter(&dummy_1).is_some());

    let dummy_2 = Info {
        keywords: vec![],
        categories: vec![],
        last_update: DateTime::<Utc>::MIN_UTC,
        created_at: DateTime::<Utc>::MIN_UTC,
        description: None,
        version: String::new(),
        stable_version: None,
        downloads: 0,
        recent_downloads: None,
        name: "azure-test-2".to_string(),
        repo_link: None,
        docs_link: None,
    };

    assert!(filters.filter(&dummy_2).is_some());
}

impl App {
    fn new(
        info_rx: crate::Receiver,
        send_tx: tokio_mpsc::UnboundedSender<()>,
        max_screen_entries: usize,
    ) -> Self {
        let send_recv_state = SendRecvState::new(info_rx, send_tx);

        Self {
            send_recv_state,
            max_screen_entries,
            entries: Entries::new(max_screen_entries),
            table_state: ratatui::widgets::TableState::default(),
            colors: TableColors::new(&tailwind::STONE),
            seen_crates: HashSet::default(),
            filters: Filters::default(),
            prev_max_info_width: 0,
            menu_state: MenuState::default(),
            hide_footer: false,
        }
    }

    fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;

            if self.update_loop()? {
                return Ok(());
            }
        }
    }

    fn update_loop(&mut self) -> Result<bool> {
        loop {
            let mut upd_iter = self.send_recv_state.update_iter(self.max_screen_entries, &mut self.seen_crates, &self.filters);

            while let Some(info) = upd_iter.next() {
                if self.entries.insert_info(info).is_some() {
                    upd_iter.state.received_count += 1;
                }
            }

            let has_update = upd_iter.has_update;

            if has_update {
                self.send_recv_state.received_count = self.entries.entries_len();
            }

            use std::time::Duration;
            if has_update && !event::poll(Duration::ZERO)? {
                return Ok(false);
            }

            if !event::poll(Duration::from_millis(100))? {
                continue;
            }

            let ev = event::read()?;

            match &mut self.menu_state.current {
                None => {
                    if let Event::Key(key) = ev {
                        if matches!(key.kind, KeyEventKind::Press) {
                            match key.code {
                                KeyCode::Char('q') => return Ok(true),
                                KeyCode::Char('j') => {
                                    let selected = self.table_state.selected();
                                    let new_selected = self.entries.next_entry_idx(selected);
                                    self.table_state.select(new_selected);
                                }
                                KeyCode::Char('k') => {
                                    let selected = self.table_state.selected();
                                    let new_selected = self.entries.prev_entry_idx(selected);
                                    self.table_state.select(new_selected);
                                }
                                KeyCode::Char('d') => {
                                    if let Some(idx) = self.table_state.selected() {
                                        self.entries.mark_for_replacement(
                                            idx,
                                            &mut self.send_recv_state,
                                        );

                                        if self.entries.is_first_entry(idx) {
                                            let new_selected =
                                                self.entries.next_entry_idx(Some(idx));
                                            self.table_state.select(new_selected);
                                        } else {
                                            let new_selected =
                                                self.entries.prev_entry_idx(Some(idx));
                                            self.table_state.select(new_selected);
                                        }
                                    }
                                }
                                KeyCode::Char('g') | KeyCode::Char('o') => self.goto_link(),
                                KeyCode::Char('r') => {
                                    self.entries.clear_all(
                                        &mut self.send_recv_state,
                                    );
                                    self.table_state.select(None);
                                }
                                KeyCode::Char('f') => {
                                    self.menu_state.current = Some((
                                        CurrentMenu::Filters(SelectedFilter::Keywords),
                                        InputMode::Normal,
                                    ));
                                }
                                KeyCode::Char('v') => {
                                    if self.table_state.selected().is_some() {
                                        self.menu_state.current =
                                            Some((CurrentMenu::FullscreenInfo, InputMode::Normal));
                                    }
                                }
                                KeyCode::Char('h') => {
                                    self.hide_footer = !self.hide_footer;
                                }
                                KeyCode::Char('n') => {
                                    self.max_screen_entries += 1;
                                    self.entries.resize(self.max_screen_entries);
                                    self.send_recv_state.received_count = self.entries.entries_len();
                                }
                                KeyCode::Char('m') => {
                                    self.max_screen_entries -= 1;
                                    self.entries.resize(self.max_screen_entries);
                                    self.send_recv_state.received_count = self.entries.entries_len();
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Some((CurrentMenu::FullscreenInfo, _)) => {
                    if let Event::Key(key) = ev {
                        if matches!(key.kind, KeyEventKind::Press) {
                            match key.code {
                                KeyCode::Char('d') => {
                                    if let Some(idx) = self.table_state.selected() {
                                        self.menu_state.current = None;
                                        self.entries.mark_for_replacement(
                                            idx,
                                            &mut self.send_recv_state,
                                        );

                                        if self.entries.is_first_entry(idx) {
                                            let new_selected =
                                                self.entries.next_entry_idx(Some(idx));
                                            self.table_state.select(new_selected);
                                        } else {
                                            let new_selected =
                                                self.entries.prev_entry_idx(Some(idx));
                                            self.table_state.select(new_selected);
                                        }
                                    }
                                }
                                KeyCode::Char('j') => {
                                    if !key.modifiers.contains(KeyModifiers::SHIFT) {
                                        self.menu_state.current = None;
                                    }
                                    let selected = self.table_state.selected();
                                    let new_selected = self.entries.next_entry_idx(selected);
                                    self.table_state.select(new_selected);
                                }
                                KeyCode::Char('k') => {
                                    if !key.modifiers.contains(KeyModifiers::SHIFT) {
                                        self.menu_state.current = None;
                                    }
                                    let selected = self.table_state.selected();
                                    let new_selected = self.entries.prev_entry_idx(selected);
                                    self.table_state.select(new_selected);
                                }
                                KeyCode::Char('g') | KeyCode::Char('o') => self.goto_link(),
                                KeyCode::Char('r') => {
                                    self.menu_state.current = None;
                                    self.entries.clear_all(
                                        &mut self.send_recv_state,
                                    );
                                    self.table_state.select(None);
                                }
                                KeyCode::Esc => {
                                    self.menu_state.current = None;
                                }
                                _ => (),
                            }
                        }
                    }
                }
                Some((cur, mode @ InputMode::Normal)) => {
                    if let Event::Key(key) = ev {
                        if matches!(key.kind, KeyEventKind::Press) {
                            match key.code {
                                KeyCode::Char('i') => *mode = InputMode::Editing,
                                KeyCode::Char('j') => cur.clone().move_down_input(self),
                                KeyCode::Char('k') => cur.clone().move_up_input(self),
                                KeyCode::Esc => self.menu_state.current = None,
                                _ => (),
                            }
                        }
                    }
                }
                Some((cur, mode @ InputMode::Editing)) => {
                    if let Event::Key(key) = ev {
                        if matches!(key.kind, KeyEventKind::Press) {
                            match key.code {
                                KeyCode::Enter | KeyCode::Esc => {
                                    *mode = InputMode::Normal;
                                    cur.clone().handle_input_enter(self);
                                }
                                KeyCode::Backspace => {
                                    cur.clone().handle_input_backspace(self);
                                }
                                KeyCode::Char(key) => cur.clone().handle_input_char(self, key),
                                _ => (),
                            }
                        }
                    }
                }
            }
            return Ok(false);
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let footer_height = u16::from(!self.hide_footer) * 3;
        let vertical = &Layout::vertical([Constraint::Min(5), Constraint::Length(footer_height)]);
        let rects = vertical.split(frame.area());

        let area = rects[0];

        let header_style = Style::default()
            .fg(self.colors.header_fg)
            .bg(self.colors.header_bg);
        let selected_row_style = Style::default()
            .add_modifier(Modifier::REVERSED)
            .fg(self.colors.selected_row_style_fg);
        let selected_col_style = Style::default().fg(self.colors.selected_column_style_fg);
        let selected_cell_style = Style::default()
            .add_modifier(Modifier::REVERSED)
            .fg(self.colors.selected_cell_style_fg);

        let headers = ["Name", "Description", "Keywords", "Categories"];
        let column_count = headers.len();

        let mut max_info_width = 0;

        let header = headers
            .into_iter()
            .map(Cell::from)
            .collect::<Row>()
            .style(header_style)
            .height(1);

        let now = chrono::Utc::now();

        let rows = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, data)| {
                let color = match i % 2 {
                    0 => self.colors.normal_row_color,
                    _ => self.colors.alt_row_color,
                };

                let total_width = usize::try_from(area.width).unwrap();
                let desc_width = total_width
                    - self.prev_max_info_width
                    - self.entries.longest_keyword_len
                    - self.entries.longest_category_len
                    - column_count
                    - 3;

                let max_newlines = 2;

                let (row, height, width) = match data {
                    Ok(info) => info.render(max_newlines, desc_width, &now),
                    Err(Some(prev)) => prev.render_clear(max_newlines, desc_width, &now),
                    Err(None) => (Row::default(), 1, 0),
                };

                max_info_width = core::cmp::max(max_info_width, width);

                row.style(Style::new().fg(self.colors.row_fg).bg(color))
                    .height(u16::try_from(height + 1).unwrap())
            })
            .collect::<Vec<_>>();

        let t = Table::new(
            rows,
            [
                // + 1 is for padding.
                Constraint::Length(u16::try_from(max_info_width + 1).unwrap()),
                Constraint::Fill(1),
                Constraint::Length(u16::try_from(self.entries.longest_keyword_len + 1).unwrap()),
                Constraint::Length(u16::try_from(self.entries.longest_category_len + 1).unwrap()),
            ],
        )
        .header(header)
        .row_highlight_style(selected_row_style)
        .column_highlight_style(selected_col_style)
        .cell_highlight_style(selected_cell_style)
        .bg(self.colors.buffer_bg)
        .highlight_spacing(HighlightSpacing::Always)
        .flex(Flex::Legacy);
        frame.render_stateful_widget(t, area, &mut self.table_state);

        self.prev_max_info_width = max_info_width;

        if let Some((current, mode)) = &self.menu_state.current {
            let title = current.popup_title();

            let block = Block::bordered()
                .title(title)
                .bg(self.colors.buffer_bg)
                .fg(self.colors.row_fg);
            let area = popup_area(area, 60, 80);
            frame.render_widget(Clear, area); //this clears out the background

            let inner_area = block.inner(area);
            frame.render_widget(block, area);

            current
                .clone()
                .render_menu(mode, inner_area, &now, frame, self);
        }

        let footer_area = rects[1];

        let mut footer_info = String::new();

        if matches!(self.menu_state.current, None) {
            footer_info.push_str("q: quit");
        } else {
            footer_info.push_str("Esc: exit");
        }

        let has_entries = match self.menu_state.current {
            None => self.table_state.selected().is_some(),
            Some((CurrentMenu::FullscreenInfo, _)) => true,
            _ => false,
        };

        if has_entries {
            footer_info.push_str(", d: delete entry, r: clear entries, g/o: goto repo 🌐");
            if matches!(self.menu_state.current, None) {
                footer_info.push_str(", v: full info");
            }
        }

        if matches!(
            self.menu_state.current,
            None | Some((CurrentMenu::Filters(_), InputMode::Normal))
        ) {
            footer_info.push_str(", k: up, j: down");
        }
        if matches!(
            self.menu_state.current,
            Some((CurrentMenu::FullscreenInfo, _))
        ) {
            footer_info.push_str(", k: up (exits), j: down (exits), ⇧ + j/k: no exit");
        }

        if matches!(self.menu_state.current, None) && has_entries {
            footer_info.push_str(", f: open filters, h: hide footer");
        }

        if matches!(
            self.menu_state.current,
            Some((CurrentMenu::Filters(_), InputMode::Normal))
        ) {
            footer_info.push_str(", i: edit text");
        }

        if matches!(
            self.menu_state.current,
            Some((CurrentMenu::Filters(_), InputMode::Editing))
        ) {
            footer_info.push_str(", Enter: update");
        }

        if matches!(
            self.menu_state.current,
            None
        ) {
            footer_info.push_str(&format!(", current entries: {} (n: inc, m: dec)", self.max_screen_entries));
        }

        let footer = Paragraph::new(Text::from(footer_info.as_str()))
            .style(
                Style::new()
                    .fg(self.colors.row_fg)
                    .bg(self.colors.buffer_bg),
            )
            .centered()
            .block(
                Block::bordered().border_style(Style::new().fg(self.colors.footer_border_color)),
            );
        if !self.hide_footer {
            frame.render_widget(footer, footer_area);
        }
    }

    fn goto_link(&self) {
        if let Some(Some(info)) = self.table_state.selected().map(|s| self.entries.get(s)) {
            let mut url = None;

            if let Some(repo) = info.repo_link.as_ref() {
                url = Some(repo);
            } else if let Some(docs) = info.docs_link.as_ref() {
                url = Some(docs);
            };

            if let Some(url) = url {
                open::that_in_background(url);
            }
        }
    }
}

use ratatui::style::{Color, Modifier, Style, palette::tailwind};

struct TableColors {
    buffer_bg: Color,
    header_bg: Color,
    header_fg: Color,
    row_fg: Color,
    selected_row_style_fg: Color,
    selected_column_style_fg: Color,
    selected_cell_style_fg: Color,
    normal_row_color: Color,
    alt_row_color: Color,
    footer_border_color: Color,
}

impl TableColors {
    const fn new(color: &tailwind::Palette) -> Self {
        Self {
            buffer_bg: color.c950,
            header_bg: color.c800,
            header_fg: color.c200,
            row_fg: color.c200,
            selected_row_style_fg: color.c400,
            selected_column_style_fg: color.c400,
            selected_cell_style_fg: color.c600,
            normal_row_color: color.c950,
            alt_row_color: color.c900,
            footer_border_color: color.c400,
        }
    }
}

/// helper function to create a centered rect using up certain percentage of the available rect `r`
fn popup_area(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::vertical([Constraint::Percentage(percent_y)]).flex(Flex::Center);
    let horizontal = Layout::horizontal([Constraint::Percentage(percent_x)]).flex(Flex::Center);
    let [area] = vertical.areas(area);
    let [area] = horizontal.areas(area);
    area
}

fn format_downloads(num: u64) -> String {
    fn format_decimal(rem: u64) -> u64 {
        let mut dec = rem / 100;

        if dec != 9 && rem / 10 >= 5 {
            dec += 1;
        }
        dec
    }

    if num < 1000 {
        format!("{num}")
    } else if num < 100_000 {
        let main = num / 1000;
        let rem = num % 1000;

        let dec = format_decimal(rem);

        format!("{main}.{dec}K")
    } else if num < 1_000_000 {
        format!("{}K", num / 1000)
    } else if num < 100_000_000 {
        let main = num / 1_000_000;
        let rem = num % 1_000_000;

        let dec = format_decimal(rem / 1000);

        format!("{main}.{dec}M")
    } else if num < 1_000_000_000 {
        format!("{}M", num / 1_000_000)
    } else if num < 100_000_000_000 {
        let main = num / 1_000_000_000;
        let rem = num % 1_000_000_000;

        let dec = format_decimal(rem / 1_000_000);

        format!("{main}.{dec}B")
    } else {
        todo!()
    }
}

fn format_time_duration(
    then: &chrono::DateTime<chrono::Utc>,
    now: &chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let diff = now.signed_duration_since(then).to_std().ok()?;

    use millisecond::prelude::*;
    Some(format!(
        "{}",
        diff.pretty_with(millisecond::MillisecondOption {
            dominant_only: true,
            ..Default::default()
        })
    ))
}

impl Info {
    fn accumulate_info(&self, now: &chrono::DateTime<chrono::Utc>) -> String {
        let mut info = String::new();

        let mut kw_iter = self.keywords.iter();

        if let Some(kw) = kw_iter.next() {
            info.push_str("keywords: ");
            info.push_str(kw);

            while let Some(kw) = kw_iter.next() {
                info.push_str(", ");
                info.push_str(kw);
            }
            info.push('\n');
        }

        let mut cat_iter = self.categories.iter();

        if let Some(cat) = cat_iter.next() {
            info.push_str("categories: ");
            info.push_str(cat);

            while let Some(cat) = cat_iter.next() {
                info.push_str(", ");
                info.push_str(cat);
            }
            info.push('\n')
        }

        if let Some(recent) = self.recent_downloads {
            let downloads = format_downloads(recent);
            info.push_str("recent downloads: ");
            info.push_str(&downloads);
            info.push_str("\r\n");
        }

        let total_downloads = format_downloads(self.downloads);
        info.push_str("total downloads: ");
        info.push_str(&total_downloads);
        info.push_str("\r\n");

        use chrono::format::strftime::StrftimeItems;
        let fmt = StrftimeItems::new("%Y-%m-%d");

        info.push_str("last update: ");
        let last_update = self.last_update.date_naive();
        let upd = last_update.format_with_items(fmt.clone()).to_string();
        info.push_str(&upd);

        if let Some(dur) = format_time_duration(&self.last_update, now) {
            info.push_str(" (");
            info.push_str(&dur);
            info.push_str(" ago)\r\n");
        }

        info.push_str("created ");
        let created_at = self.created_at.date_naive();
        let upd = created_at.format_with_items(fmt).to_string();
        info.push_str(&upd);

        if let Some(dur) = format_time_duration(&self.created_at, now) {
            info.push_str(" (");
            info.push_str(&dur);
            info.push_str(" ago)\r\n");
        }

        if let Some(repo) = self.repo_link.as_ref() {
            info.push_str("repo link: ");
            info.push_str(repo);
            info.push_str("\r\n");
        }

        if let Some(stable) = self.stable_version.as_ref() {
            info.push_str("most stable version: ");
            info.push_str(stable);
            info.push_str("\r\n");
        }

        info.push_str("most recent version: ");
        info.push_str(self.version.as_ref());
        info.push_str("\r\n");
        info
    }

    fn render_full(&self, now: &chrono::DateTime<chrono::Utc>, area: Rect, frame: &mut Frame) {
        let info = self.accumulate_info(now);
        let info_lines = info.lines().count();
        let vertical = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(u16::try_from(info_lines + 1).unwrap()),
            Constraint::Fill(1),
        ]);
        let [crate_area, info_area, desc_area] = vertical.areas(area);

        frame.render_widget(self.name.as_str().bold(), crate_area);

        frame.render_widget(Paragraph::new(Text::from(info)), info_area);

        if let Some(desc) = self.description.as_ref() {
            let desc = Paragraph::new(Text::from(desc.as_str()))
                .wrap(Wrap { trim: true })
                .block(Block::bordered().title("Description"));
            frame.render_widget(desc, desc_area);
        }
    }

    fn render(
        &self,
        max_newlines: usize,
        desc_width: usize,
        now: &chrono::DateTime<chrono::Utc>,
    ) -> (Row<'_>, usize, usize) {
        let name = self.name.as_str();

        let desc = self
            .description
            .as_ref()
            .map(|desc| desc.as_str())
            .unwrap_or_default();

        let desc = textwrap::wrap(desc, desc_width)
            .into_iter()
            .map(|s| ratatui::prelude::Line::from(s))
            .collect::<Vec<_>>();
        let desc_rows = core::cmp::min(max_newlines, desc.len());
        let desc = desc[..desc_rows].to_vec();

        let kw_len = core::cmp::min(max_newlines, self.keywords.len());
        let keywords = self.keywords[..kw_len].join("\n");
        let cat_len = core::cmp::min(max_newlines, self.categories.len());
        let categories = self.categories[..cat_len].join("\n");

        let (recent_data, width) = self.format_recent_data(&now);

        let info = format!("{name}\n{recent_data}");

        let height = core::cmp::max(core::cmp::max(kw_len, cat_len), desc_rows);

        (
            Row::new(vec![
                Cell::from(Text::from(info)),
                Cell::from(Text::from(desc)),
                Cell::from(keywords),
                Cell::from(categories),
            ]),
            height,
            width,
        )
    }

    fn format_recent_data(&self, now: &chrono::DateTime<chrono::Utc>) -> (String, usize) {
        let mut recent = format_time_duration(&self.last_update, now).unwrap_or_default();

        if let Some(downloads) = self.recent_downloads {
            let num = format_downloads(downloads);
            if recent.is_empty() {
                recent = format!("{num}")
            } else {
                recent.push_str(" - ");
                recent.push_str(&num);
            }
        }

        let version = self.stable_version.as_ref().unwrap_or(&self.version);

        if recent.is_empty() {
            recent = format!("{version}");
        } else {
            recent.push_str(" - ");
            recent.push_str(version);
        }

        let max_width = core::cmp::max(self.name.len(), recent.len());
        (recent, max_width)
    }

    fn render_clear(
        &self,
        max_newlines: usize,
        desc_width: usize,
        now: &chrono::DateTime<chrono::Utc>,
    ) -> (Row<'_>, usize, usize) {
        let name = self.name.as_str();

        let desc = self
            .description
            .as_ref()
            .map(|desc| desc.as_str())
            .unwrap_or_default();

        let desc = textwrap::wrap(desc, desc_width)
            .into_iter()
            .map(|s| ratatui::prelude::Line::from(s))
            .collect::<Vec<_>>();
        let desc_rows = core::cmp::min(max_newlines, desc.len());
        let desc = desc[..desc_rows].to_vec();

        let kw_len = core::cmp::min(max_newlines, self.keywords.len());
        let keywords = self.keywords[..kw_len].join("\n");
        let cat_len = core::cmp::min(max_newlines, self.categories.len());
        let categories = self.categories[..cat_len].join("\n");

        let (recent_data, width) = self.format_recent_data(&now);

        let info = format!("{name}\n{recent_data}");

        let height = core::cmp::max(core::cmp::max(kw_len, cat_len), desc_rows);

        let strikethrough = Style::new().add_modifier(Modifier::CROSSED_OUT);

        (
            Row::new(vec![
                Cell::from(Text::from(info)).style(strikethrough),
                Cell::from(Text::from(desc)).style(strikethrough),
                Cell::from(keywords).style(strikethrough),
                Cell::from(categories).style(strikethrough),
            ]),
            height,
            width,
        )
    }
}

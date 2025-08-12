#![deny(rust_2018_idioms)]

use std::fs;
use std::path::Path;

use color::Color;
use framebuffer::{Framebuffer, KdMode, VarScreeninfo};
use freedesktop_desktop_entry::DesktopEntry;
use input::{InputStream, Key};
use termion::raw::IntoRawMode;
use thiserror::Error;

const USERNAME_CAP: usize = 64;
const PASSWORD_CAP: usize = 64;

// from linux/fb.h
const FB_ACTIVATE_NOW: u32 = 0;
const FB_ACTIVATE_FORCE: u32 = 128;

mod buffer;
mod color;
mod draw;
mod greetd;
mod input;

#[derive(PartialEq, Copy, Clone)]
enum Mode {
    SelectingSession,
    EditingUsername,
    EditingPassword,
}

#[derive(Error, Debug)]
#[non_exhaustive]
enum Error {
    #[error("Error performing buffer operation: {0}")]
    Buffer(#[from] buffer::BufferError),
    #[error("Error performing draw operation: {0}")]
    Draw(#[from] draw::DrawError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

struct Target {
    name: String,
    exec: Vec<String>,
}

impl Target {
    fn load<P: AsRef<Path>>(path: P) -> Option<Self> {
        let path = path.as_ref();
        let data = fs::read_to_string(path).ok()?;
        let entry = DesktopEntry::decode(path, &data).ok()?;

        let cmdline = entry.exec()?;
        let exec = shell_words::split(cmdline).ok()?;

        let name = entry.name(None).unwrap_or(entry.appid.into()).into_owned();

        Some(Self { name, exec })
    }
}

struct LoginManager<'a> {
    buf: &'a mut [u8],
    device: &'a fs::File,

    headline_font: draw::Font,
    prompt_font: draw::Font,

    screen_size: (u32, u32),
    dimensions: (u32, u32),
    mode: Mode,
    greetd: greetd::GreetD,
    targets: Vec<Target>,
    target_index: usize,
    cursor_pos: usize,

    var_screen_info: &'a VarScreeninfo,
    should_refresh: bool,
}

impl<'a> LoginManager<'a> {
    fn new(
        fb: &'a mut Framebuffer,
        screen_size: (u32, u32),
        dimensions: (u32, u32),
        greetd: greetd::GreetD,
        targets: Vec<Target>,
    ) -> Self {
        Self {
            buf: &mut fb.frame,
            device: &fb.device,
            headline_font: draw::Font::new(&draw::DEJAVUSANS_MONO, 72.0),
            prompt_font: draw::Font::new(&draw::DEJAVUSANS_MONO, 32.0),
            screen_size,
            dimensions,
            mode: Mode::EditingUsername,
            greetd,
            targets,
            target_index: 1, // TODO: remember last user selection
            cursor_pos: 0,
            var_screen_info: &fb.var_screen_info,
            should_refresh: false,
        }
    }

    fn refresh(&mut self) {
        if self.should_refresh {
            self.should_refresh = false;
            let mut screeninfo = self.var_screen_info.clone();
            screeninfo.activate |= FB_ACTIVATE_NOW | FB_ACTIVATE_FORCE;
            Framebuffer::put_var_screeninfo(self.device, &screeninfo)
                .expect("Failed to refresh framebuffer");
        }
    }

    fn clear(&mut self) {
        let mut buf = buffer::Buffer::new(self.buf, self.screen_size);
        let bg = Color::BLACK;
        buf.memset(&bg);
        self.should_refresh = true;
    }

    fn offset(&self) -> (u32, u32) {
        (
            (self.screen_size.0 - self.dimensions.0) / 2,
            (self.screen_size.1 - self.dimensions.1) / 2,
        )
    }

    fn draw_bg(&mut self, box_color: &Color) -> Result<(), Error> {
        let (x, y) = self.offset();
        let mut buf = buffer::Buffer::new(self.buf, self.screen_size);
        let bg = Color::BLACK;
        let fg = Color::WHITE;

        draw::draw_box(
            &mut buf.subdimensions((x, y, self.dimensions.0, self.dimensions.1))?,
            box_color,
            (self.dimensions.0, self.dimensions.1),
        )?;

        let hostname = hostname::get()?.to_string_lossy().into_owned();

        self.headline_font.auto_draw_text(
            &mut buf.offset(((self.screen_size.0 / 2) - 300, 32))?,
            &bg,
            &fg,
            &format!("Welcome to {hostname}"),
            None,
        )?;

        self.headline_font.auto_draw_text(
            &mut buf
                .subdimensions((x, y, self.dimensions.0, self.dimensions.1))?
                .offset((32, 24))?,
            &bg,
            &fg,
            "Login",
            None,
        )?;

        let (session_color, username_color, password_color) = match self.mode {
            Mode::SelectingSession => (Color::YELLOW, Color::WHITE, Color::WHITE),
            Mode::EditingUsername => (Color::WHITE, Color::YELLOW, Color::WHITE),
            Mode::EditingPassword => (Color::WHITE, Color::WHITE, Color::YELLOW),
        };

        self.prompt_font.auto_draw_text(
            &mut buf
                .subdimensions((x, y, self.dimensions.0, self.dimensions.1))?
                .offset((256, 24))?,
            &bg,
            &session_color,
            "session:",
            None,
        )?;

        self.prompt_font.auto_draw_text(
            &mut buf
                .subdimensions((x, y, self.dimensions.0, self.dimensions.1))?
                .offset((256, 64))?,
            &bg,
            &username_color,
            "username:",
            None,
        )?;

        self.prompt_font.auto_draw_text(
            &mut buf
                .subdimensions((x, y, self.dimensions.0, self.dimensions.1))?
                .offset((256, 104))
                .unwrap(),
            &bg,
            &password_color,
            "password:",
            None,
        )?;

        self.should_refresh = true;

        Ok(())
    }

    fn draw_target(&mut self) -> Result<(), Error> {
        let (x, y) = self.offset();
        let (x, y) = (x + 416, y + 24);
        let dim = (self.dimensions.0 - 416 - 32, 32);

        let mut buf = buffer::Buffer::new(self.buf, self.screen_size);
        let mut buf = buf.subdimensions((x, y, dim.0, dim.1))?;
        let bg = Color::BLACK;
        buf.memset(&bg);

        self.prompt_font.auto_draw_text(
            &mut buf,
            &bg,
            &Color::WHITE,
            &self.targets[self.target_index].name,
            None,
        )?;

        self.should_refresh = true;

        Ok(())
    }

    fn draw_username(&mut self, username: &str) -> Result<(), Error> {
        let (x, y) = self.offset();
        let (x, y) = (x + 416, y + 64);
        let dim = (self.dimensions.0 - 416 - 32, 32);

        let mut buf = buffer::Buffer::new(self.buf, self.screen_size);
        let mut buf = buf.subdimensions((x, y, dim.0, dim.1))?;
        let bg = Color::BLACK;
        buf.memset(&bg);

        let cursor_pos = (self.mode == Mode::EditingUsername).then_some(self.cursor_pos);
        self.prompt_font
            .auto_draw_text(&mut buf, &bg, &Color::WHITE, username, cursor_pos)?;

        self.should_refresh = true;

        Ok(())
    }

    fn draw_password(&mut self, password: &str) -> Result<(), Error> {
        let (x, y) = self.offset();
        let (x, y) = (x + 416, y + 104);
        let dim = (self.dimensions.0 - 416 - 32, 32);

        let mut buf = buffer::Buffer::new(self.buf, self.screen_size);
        let mut buf = buf.subdimensions((x, y, dim.0, dim.1))?;
        let bg = Color::BLACK;
        buf.memset(&bg);

        let mut stars = "".to_string();
        for _ in 0..password.len() {
            stars += "*";
        }

        let cursor_pos = (self.mode == Mode::EditingPassword).then_some(self.cursor_pos);

        self.prompt_font
            .auto_draw_text(&mut buf, &bg, &Color::WHITE, &stars, cursor_pos)?;

        self.should_refresh = true;

        Ok(())
    }

    fn goto_next_mode(&mut self) {
        self.mode = match self.mode {
            Mode::SelectingSession => Mode::EditingUsername,
            Mode::EditingUsername => Mode::EditingPassword,
            Mode::EditingPassword => Mode::SelectingSession,
        }
    }

    fn goto_prev_mode(&mut self) {
        self.mode = match self.mode {
            Mode::SelectingSession => Mode::EditingPassword,
            Mode::EditingUsername => Mode::SelectingSession,
            Mode::EditingPassword => Mode::EditingUsername,
        }
    }

    fn greeter_loop(&mut self) {
        let mut username = String::with_capacity(USERNAME_CAP);
        let mut password = String::with_capacity(PASSWORD_CAP);
        let mut last_username_len = username.len();
        let mut last_password_len = password.len();
        let mut last_cursor_pos = 1; // this forces the first iteration to draw the user/pass so a cursor is drawn
        let mut last_target_index = self.target_index;
        let mut last_mode = self.mode;
        let mut had_failure = false;

        let mut input = InputStream::new();

        self.draw_target().expect("unable to draw target session");

        loop {
            let max_cursor_pos = match self.mode {
                Mode::SelectingSession => 0,
                Mode::EditingUsername => username.len(),
                Mode::EditingPassword => password.len(),
            };
            self.cursor_pos = self.cursor_pos.min(max_cursor_pos);

            let mode_changed = last_mode != self.mode;
            if mode_changed {
                self.cursor_pos = max_cursor_pos;
                self.draw_bg(&Color::GRAY)
                    .expect("unable to draw background");
            }

            let cursor_moved = last_cursor_pos != self.cursor_pos;

            if username.len() != last_username_len || mode_changed || cursor_moved {
                self.draw_username(&username)
                    .expect("unable to draw username prompt");
            }
            if password.len() != last_password_len || mode_changed || cursor_moved {
                self.draw_password(&password)
                    .expect("unable to draw username prompt");
            }
            if last_target_index != self.target_index {
                self.draw_target().expect("unable to draw target session");
            }

            if had_failure {
                self.draw_bg(&Color::GRAY)
                    .expect("unable to draw background");
            }

            last_username_len = username.len();
            last_password_len = password.len();
            last_mode = self.mode;
            last_target_index = self.target_index;
            last_cursor_pos = self.cursor_pos;
            had_failure = false;

            match input.next() {
                Key::CtrlK | Key::CtrlU => match self.mode {
                    Mode::SelectingSession => (),
                    Mode::EditingUsername => username.clear(),
                    Mode::EditingPassword => password.clear(),
                },
                Key::CtrlC | Key::CtrlD => {
                    username.clear();
                    password.clear();
                    self.greetd.cancel();
                    return;
                }
                k @ (Key::Backspace | Key::Delete) => {
                    let field = match self.mode {
                        Mode::EditingUsername => &mut username,
                        Mode::EditingPassword => &mut password,
                        Mode::SelectingSession => continue,
                    };
                    if k == Key::Backspace {
                        if self.cursor_pos == 0 {
                            continue;
                        }
                        self.retreat_cursor(field);
                    }
                    if self.cursor_pos < field.len() {
                        field.remove(self.cursor_pos);
                    }
                }
                Key::Return => match self.mode {
                    Mode::SelectingSession => self.mode = Mode::EditingUsername,
                    Mode::EditingUsername => {
                        if !username.is_empty() {
                            self.mode = Mode::EditingPassword;
                        }
                    }
                    Mode::EditingPassword => {
                        if password.is_empty() {
                            username.clear();
                            self.mode = Mode::EditingUsername;
                        } else {
                            self.draw_bg(&Color::YELLOW)
                                .expect("unable to draw background");
                            let res = self.greetd.login(
                                username,
                                password,
                                self.targets[self.target_index].exec.clone(),
                            );
                            username = String::with_capacity(USERNAME_CAP);
                            password = String::with_capacity(PASSWORD_CAP);
                            match res {
                                Ok(_) => return,
                                Err(_) => {
                                    self.draw_bg(&Color::RED)
                                        .expect("unable to draw background");
                                    self.mode = Mode::EditingUsername;
                                    self.greetd.cancel();
                                    had_failure = true;
                                }
                            }
                        }
                    }
                },
                Key::Up => self.goto_prev_mode(),
                Key::Down | Key::Tab => self.goto_next_mode(),
                Key::Right => match self.mode {
                    Mode::SelectingSession => {
                        self.target_index = (self.target_index + 1) % self.targets.len()
                    }
                    Mode::EditingUsername => self.advance_cursor(&username),
                    Mode::EditingPassword => self.advance_cursor(&password),
                },
                Key::Left => match self.mode {
                    Mode::SelectingSession => {
                        if self.target_index == 0 {
                            self.target_index = self.targets.len();
                        }
                        self.target_index -= 1;
                    }
                    Mode::EditingUsername => self.retreat_cursor(&username),
                    Mode::EditingPassword => self.retreat_cursor(&password),
                },
                Key::Other(k) => {
                    let field = match self.mode {
                        Mode::EditingUsername => &mut username,
                        Mode::EditingPassword => &mut password,
                        Mode::SelectingSession => continue,
                    };
                    // TODO: proper unicode input?
                    let ch = k as char;
                    field.insert(self.cursor_pos, ch);
                    self.cursor_pos += ch.len_utf8();
                }
                Key::OtherEsc(_) | Key::OtherCsi(_) => (), // shrug
            }
            self.refresh();
        }
    }

    fn retreat_cursor(&mut self, field: &str) {
        let Some(prev_char) = field[..self.cursor_pos].chars().last() else {
            // the cursor is already at the start of the field
            return;
        };
        self.cursor_pos -= prev_char.len_utf8();
    }

    fn advance_cursor(&mut self, field: &str) {
        let Some(next_char) = field[self.cursor_pos..].chars().next() else {
            // the cursor is already at the end of the field
            return;
        };
        self.cursor_pos += next_char.len_utf8();
    }
}

fn main() {
    let mut framebuffer = Framebuffer::new("/dev/fb0").expect("unable to open framebuffer device");

    let w = framebuffer.var_screen_info.xres;
    let h = framebuffer.var_screen_info.yres;

    let raw = std::io::stdout()
        .into_raw_mode()
        .expect("unable to enter raw mode");

    let _ = Framebuffer::set_kd_mode(KdMode::Graphics).expect("unable to enter graphics mode");

    let greetd = greetd::GreetD::new();

    let targets = ["/usr/share/wayland-sessions", "/usr/share/xsessions"]
        .iter()
        .flat_map(fs::read_dir)
        .flatten()
        .flatten()
        .flat_map(|dir_entry| Target::load(dir_entry.path()))
        .collect();

    let mut lm = LoginManager::new(&mut framebuffer, (w, h), (1024, 168), greetd, targets);

    lm.clear();
    lm.draw_bg(&Color::GRAY).expect("unable to draw background");
    lm.refresh();

    lm.greeter_loop();
    let _ = Framebuffer::set_kd_mode(KdMode::Text).expect("unable to leave graphics mode");
    drop(raw);
}

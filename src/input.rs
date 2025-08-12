use std::io::{Bytes, Read, StdinLock};

use framebuffer::{Framebuffer, KdMode};

pub struct InputStream {
    inner: Bytes<StdinLock<'static>>,
}

impl InputStream {
    pub fn new() -> Self {
        Self {
            inner: std::io::stdin().lock().bytes(),
        }
    }

    fn next_byte(&mut self) -> u8 {
        let Some(Ok(byte)) = self.inner.next() else {
            Framebuffer::set_kd_mode(KdMode::Text).expect("unable to leave graphics mode");
            std::process::exit(1);
        };
        byte
    }

    pub fn next(&mut self) -> Key {
        match self.next_byte() {
            0x03 => Key::CtrlC,
            0x04 => Key::CtrlD,
            0x0B => Key::CtrlK,
            0x15 => Key::CtrlU,
            0x7F => Key::Backspace,
            b'\t' => Key::Tab,
            b'\r' => Key::Return,
            0x1B => match self.next_byte() {
                b'[' => match self.next_byte() {
                    b'A' => Key::Up,
                    b'B' => Key::Down,
                    b'C' => Key::Right,
                    b'D' => Key::Left,
                    b => Key::OtherCsi(b),
                },
                b => Key::OtherEsc(b),
            },
            b => Key::Other(b),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum Key {
    CtrlK = 0x0B,
    CtrlU = 0x15,
    CtrlC = 0x03,
    CtrlD = 0x04,
    Backspace = 0x7F,
    Tab = b'\t',
    Return = b'\r',
    Up,
    Down,
    Left,
    Right,
    Other(u8),
    // Not an ideal way of representing things, but should get the job done.
    OtherEsc(u8),
    OtherCsi(u8),
}

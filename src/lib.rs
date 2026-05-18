//! Async interface for K1EL WinKeyer 3 (WK3) Morse/CW keyers.
//!
//! WK3 powers up at 1200 baud, 8 data bits, no parity, 2 stop bits.  Use
//! [`WinKeyer::open`] to enter host mode, then send commands/text.

use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{sleep, timeout};
use tokio_serial::{DataBits, FlowControl, Parity, SerialPortBuilderExt, SerialStream, StopBits};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);
const POLL_DELAY: Duration = Duration::from_millis(50);

/// WK3 command bytes.
pub mod command {
    pub const ADMIN: u8 = 0x00;
    pub const SET_WPM: u8 = 0x02;
    pub const SET_WEIGHTING: u8 = 0x03;
    pub const SET_PTT_LEAD_TAIL: u8 = 0x04;
    pub const SET_MODE: u8 = 0x0e;
    pub const REQUEST_STATUS: u8 = 0x15;
    pub const PTT_ON_OFF: u8 = 0x18;
    pub const CLEAR_BUFFER: u8 = 0x0a;
}

/// WK3 administrative subcommands.
pub mod admin {
    pub const RESET: u8 = 0x01;
    pub const HOST_OPEN: u8 = 0x02;
    pub const HOST_CLOSE: u8 = 0x03;
    pub const ECHO_TEST: u8 = 0x04;
    pub const GET_FW_MAJOR_REV: u8 = 0x09;
    pub const SET_WK1_MODE: u8 = 0x0a;
    pub const SET_WK2_MODE: u8 = 0x0b;
    pub const SET_LOW_BAUD: u8 = 0x11;
    pub const SET_HIGH_BAUD: u8 = 0x12;
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("serial port error: {0}")]
    Serial(#[from] tokio_serial::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("operation timed out")]
    Timeout,
    #[error("WPM must be in the range 5..=99, got {0}")]
    InvalidWpm(u8),
    #[error("weighting must be in the range 10..=90, got {0}")]
    InvalidWeighting(u8),
    #[error("expected WK status byte, got 0x{0:02x}")]
    UnexpectedByte(u8),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Parsed WK status byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub wait: bool,
    pub key_down: bool,
    pub busy: bool,
    pub break_in: bool,
    pub xoff: bool,
    pub raw: u8,
}

impl Status {
    pub fn from_byte(byte: u8) -> Result<Self> {
        // Status bytes are tagged with the three MSBs 110.
        if byte & 0xe0 != 0xc0 {
            return Err(Error::UnexpectedByte(byte));
        }
        Ok(Self {
            wait: byte & 0x10 != 0,
            key_down: byte & 0x08 != 0,
            busy: byte & 0x04 != 0,
            break_in: byte & 0x02 != 0,
            xoff: byte & 0x01 != 0,
            raw: byte,
        })
    }
}

/// Async WK3 host-mode connection.
pub struct WinKeyer {
    port: SerialStream,
    timeout: Duration,
    is_open: bool,
}

impl WinKeyer {
    /// Open a serial device and enter WK host mode.
    ///
    /// Returns the revision byte sent by WK3 in response to `Admin:Host Open`.
    pub async fn open(path: impl AsRef<Path>) -> Result<(Self, u8)> {
        let builder = tokio_serial::new(path.as_ref().to_string_lossy(), 1200)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::Two)
            .flow_control(FlowControl::None);
        let port = builder.open_native_async()?;
        let mut wk = Self {
            port,
            timeout: DEFAULT_TIMEOUT,
            is_open: false,
        };
        let rev = wk.host_open().await?;
        Ok((wk, rev))
    }

    /// Change the command/read timeout used by this connection.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    async fn host_open(&mut self) -> Result<u8> {
        self.write_all(&[command::ADMIN, admin::HOST_OPEN]).await?;
        let rev = self.read_byte().await?;
        self.is_open = true;
        Ok(rev)
    }

    /// Leave host mode. Call this before exiting so standalone settings are restored.
    pub async fn close(&mut self) -> Result<()> {
        if self.is_open {
            self.write_all(&[command::ADMIN, admin::HOST_CLOSE]).await?;
            self.port.flush().await?;
            self.is_open = false;
        }
        Ok(())
    }

    /// Clear Buffer. Reset Input Buffer Pointers
    pub async fn clear_buffer(&mut self) -> Result<()> {
        self.write_all(&[command::CLEAR_BUFFER]).await
    }

    /// Set immediate Morse sending speed, 5..=99 WPM.
    pub async fn set_wpm(&mut self, wpm: u8) -> Result<()> {
        if !(5..=99).contains(&wpm) {
            return Err(Error::InvalidWpm(wpm));
        }
        self.write_all(&[command::SET_WPM, wpm]).await
    }

    /// Set immediate weighting, 10..=90 percent. 50 is nominal.
    pub async fn set_weighting(&mut self, weighting: u8) -> Result<()> {
        if !(10..=90).contains(&weighting) {
            return Err(Error::InvalidWeighting(weighting));
        }
        self.write_all(&[command::SET_WEIGHTING, weighting]).await
    }

    /// Set PTT lead/tail values in 10 ms units, each 0..=250.
    pub async fn set_ptt_lead_tail(&mut self, lead: u8, tail: u8) -> Result<()> {
        self.write_all(&[command::SET_PTT_LEAD_TAIL, lead, tail])
            .await
    }

    /// Request and read current status.
    pub async fn status(&mut self) -> Result<Status> {
        self.write_all(&[command::REQUEST_STATUS]).await?;
        self.read_status().await
    }

    /// Send text through the WK input buffer.
    ///
    /// Newlines and tabs are converted to spaces. Other bytes are sent as-is;
    /// WK3 ignores unsupported ASCII bytes.
    pub async fn send_text(&mut self, text: impl AsRef<str>) -> Result<()> {
        for byte in text.as_ref().bytes().map(normalize_text_byte) {
            self.wait_for_buffer_space().await?;
            self.write_all(&[byte]).await?;
        }
        Ok(())
    }

    /// Wait until the keyer reports neither busy nor waiting.
    pub async fn wait_until_idle(&mut self) -> Result<()> {
        loop {
            let status = self.status().await?;
            if !status.busy && !status.wait {
                return Ok(());
            }
            sleep(POLL_DELAY).await;
        }
    }

    async fn wait_for_buffer_space(&mut self) -> Result<()> {
        loop {
            let status = self.status().await?;
            if !status.xoff {
                return Ok(());
            }
            sleep(POLL_DELAY).await;
        }
    }

    async fn read_status(&mut self) -> Result<Status> {
        loop {
            let byte = self.read_byte().await?;
            if byte & 0xe0 == 0xc0 {
                return Status::from_byte(byte);
            }
            // Ignore echo or speed-pot bytes that may already be queued.
        }
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        timeout(self.timeout, self.port.write_all(bytes))
            .await
            .map_err(|_| Error::Timeout)??;
        Ok(())
    }

    async fn read_byte(&mut self) -> Result<u8> {
        let mut buf = [0_u8; 1];
        timeout(self.timeout, self.port.read_exact(&mut buf))
            .await
            .map_err(|_| Error::Timeout)??;
        Ok(buf[0])
    }
}

impl Drop for WinKeyer {
    fn drop(&mut self) {
        // Async close cannot be awaited in Drop. Users should call close().
    }
}

fn normalize_text_byte(byte: u8) -> u8 {
    match byte {
        b'\r' | b'\n' | b'\t' => b' ',
        b'a'..=b'z' => byte.to_ascii_uppercase(),
        _ => byte,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status() {
        let s = Status::from_byte(0xc5).unwrap();
        assert!(s.busy);
        assert!(s.xoff);
        assert!(!s.wait);
    }

    #[test]
    fn validates_status_tag() {
        assert!(Status::from_byte(0x41).is_err());
    }

    #[test]
    fn normalizes_text() {
        assert_eq!(normalize_text_byte(b'a'), b'A');
        assert_eq!(normalize_text_byte(b'\n'), b' ');
    }
}

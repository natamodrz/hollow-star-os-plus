//! Serial console abstraction for early kernel output and debugging.

use core::fmt;

/// Hardware-independent serial console interface.
///
/// Board crates implement this trait for their specific UART controller
/// (for example PL011 or Mini-UART on BCM2837).
pub trait SerialConsole {
    /// Initializes the underlying UART hardware and associated pins.
    fn init(&mut self);

    /// Writes a single byte to the serial transmit queue, blocking until ready.
    fn write_byte(&mut self, byte: u8);

    /// Reads a byte from the serial receive queue if available.
    fn read_byte(&mut self) -> Option<u8>;

    /// Waits until all queued output bytes have been transmitted.
    fn flush(&mut self);

    /// Writes a UTF-8 string, normalizing `\n` into `\r\n` for serial terminals.
    fn write_str_crlf(&mut self, text: &str) {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }

    /// Returns a [`core::fmt::Write`] adapter bound to this console.
    fn writer(&mut self) -> ConsoleWriter<'_, Self> {
        ConsoleWriter(self)
    }
}

/// [`core::fmt::Write`] adapter around any [`SerialConsole`].
pub struct ConsoleWriter<'a, C: SerialConsole + ?Sized>(&'a mut C);

impl<C: SerialConsole + ?Sized> fmt::Write for ConsoleWriter<'_, C> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str_crlf(s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::SerialConsole;

    struct MockConsole {
        buf: [u8; 64],
        len: usize,
        flushed: bool,
    }

    impl MockConsole {
        const fn new() -> Self {
            Self {
                buf: [0; 64],
                len: 0,
                flushed: false,
            }
        }

        fn written(&self) -> &[u8] {
            &self.buf[..self.len]
        }
    }

    impl SerialConsole for MockConsole {
        fn init(&mut self) {}

        fn write_byte(&mut self, byte: u8) {
            if self.len < self.buf.len() {
                self.buf[self.len] = byte;
                self.len += 1;
            }
        }

        fn read_byte(&mut self) -> Option<u8> {
            None
        }

        fn flush(&mut self) {
            self.flushed = true;
        }
    }

    #[test]
    fn writer_normalizes_newlines_to_crlf() {
        let mut console = MockConsole::new();
        writeln!(console.writer(), "Delta\nKernel").unwrap();
        console.flush();

        assert_eq!(console.written(), b"Delta\r\nKernel\r\n");
        assert!(console.flushed);
    }
}

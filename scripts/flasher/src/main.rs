use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType},
};
use std::env;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

// TODO
// Use the bottom line of the terminal to be a status line
// that shows help text, and the status of the flasher
// Can be TERMINAL mode
// or FLASH mode, where we also display where we are in the flash process
// CTRL-M toggles between modes

#[derive(Debug, PartialEq, Clone, Copy)]
enum Mode {
    Terminal,
    Flash,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum FlashState {
    HssBooted,
    HssBootedPostFlash,
    HssInterruptPrompt,
    UsbHostConnecting,
    FlashComplete,
    Unknown,
}

impl Mode {
    fn status_text(&self, flash_state: &FlashState) -> String {
        match self {
            Mode::Terminal => {
                "\x1b[7mTERMINAL MODE | Ctrl-T: Exit | Ctrl-Y: Toggle Mode\x1b[0m".to_string()
            }
            Mode::Flash => format!(
                "\x1b[7mFLASH MODE | State: {} | Ctrl-T: Exit | Ctrl-Y: Toggle Mode\x1b[0m",
                match flash_state {
                    FlashState::HssBooted => "HSS Booted",
                    FlashState::HssBootedPostFlash => "HSS Booted Post Flash",
                    FlashState::HssInterruptPrompt => "Waiting for Interrupt",
                    FlashState::UsbHostConnecting => "USB Host Connecting",
                    FlashState::FlashComplete => "Flash Complete",
                    FlashState::Unknown => "Unknown",
                }
            ),
        }
    }
}

fn update_status_line(mode: &Mode, flash_state: &FlashState) -> io::Result<()> {
    let (_, height) = terminal::size()?;

    // Move to the last line
    execute!(
        io::stdout(),
        cursor::SavePosition,
        cursor::MoveTo(0, height - 1),
        Clear(ClearType::CurrentLine),
    )?;

    // Print status
    print!("{}", mode.status_text(flash_state));

    // Restore cursor position
    execute!(io::stdout(), cursor::RestorePosition)?;
    io::stdout().flush()?;

    Ok(())
}

fn spawn_reader_thread(
    mut port: Box<dyn serialport::SerialPort>,
) -> (mpsc::Receiver<FlashState>, mpsc::Sender<Mode>) {
    let (tx, rx) = mpsc::channel();
    let (mode_tx, mode_rx) = mpsc::channel();

    // Add file creation for logging
    let log_file = std::fs::File::create("serial.log").unwrap();
    let mut log_writer = std::io::BufWriter::new(log_file);

    println!(""); // Add an empty line to the terminal
                  // Get terminal size
    let (_, height) = terminal::size().unwrap();
    // Move to the second-to-last line
    // This is where we'll print the serial data
    execute!(io::stdout(), cursor::MoveTo(0, height - 2)).unwrap();

    std::thread::spawn(move || {
        let mut serial_log: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let mut serial_buf = [0u8; 1000];
        let mut current_state = FlashState::Unknown;
        let mut current_mode = Mode::Terminal;

        loop {
            // Check for mode updates
            if let Ok(new_mode) = mode_rx.try_recv() {
                current_mode = new_mode;
            }

            if let Ok(bytes_read) = port.read(&mut serial_buf) {
                if bytes_read > 0 {
                    // Write raw bytes to log file
                    log_writer.write_all(&serial_buf[..bytes_read]).unwrap();
                    log_writer.flush().unwrap();

                    // Reset to the next-to-last line
                    execute!(
                        io::stdout(),
                        cursor::MoveTo(0, height - 2),
                        Clear(ClearType::FromCursorDown)
                    )
                    .unwrap();
                    // Print whatever we have so far
                    print!("{}", current_line);

                    // Check for the problematic sequence and insert additional escape sequence if needed
                    let mut modified_data = Vec::new();
                    // A move cusor left sequence
                    let target_sequence = [0x1B, 0x5B, 0x44];

                    let mut i = 0;
                    while i < bytes_read {
                        if i + 3 <= bytes_read
                            && serial_buf[i] == target_sequence[0]
                            && serial_buf[i + 1] == target_sequence[1]
                            && serial_buf[i + 2] == target_sequence[2]
                        {
                            // Whenever we see a move cursor left sequence, HSS has already inserted a space before it, for some reason. We want to move past that space, then insert another space to clear the last character.
                            modified_data
                                .extend_from_slice(&[0x1B, 0x5B, 0x44, 0x1B, 0x5B, 0x44, 0x20]);
                        }
                        modified_data.push(serial_buf[i]);
                        i += 1;
                    }

                    let data = String::from_utf8_lossy(&modified_data);

                    // Print the modified data
                    print!("{}\n", data);
                    // Update the status line
                    update_status_line(&current_mode, &current_state).unwrap();

                    for c in data.chars() {
                        if c == '\n' {
                            // Only handle line in Flash mode
                            if current_mode == Mode::Flash {
                                if let Ok(new_state) =
                                    handle_line(current_state, &mut port, &current_line)
                                {
                                    if new_state != current_state {
                                        current_state = new_state;
                                        tx.send(current_state).unwrap();
                                    }
                                }
                            }
                            serial_log.push(current_line.clone());
                            current_line.clear();
                        } else {
                            current_line.push(c);
                        }
                    }
                }
            }
        }
    });

    (rx, mode_tx)
}

fn handle_line(
    current_state: FlashState,
    port: &mut Box<dyn serialport::SerialPort>,
    line: &str,
) -> Result<FlashState, io::Error> {
    if line.contains("PolarFire(R) SoC Hart Software Services (HSS)") {
        if current_state == FlashState::FlashComplete {
            return Ok(FlashState::HssBootedPostFlash);
        } else {
            return Ok(FlashState::HssBooted);
        }
    }
    if line.contains("Press a key to enter CLI, ESC to skip")
        && current_state != FlashState::HssBootedPostFlash
    {
        port.write_all("c\r\n".as_bytes())?;
        return Ok(FlashState::HssInterruptPrompt);
    }
    if line.contains("Type HELP for list of commands") {
        port.write_all("mmc\r\n".as_bytes())?;
        port.write_all("usbdmsc\r\n".as_bytes())?;
        return Ok(FlashState::UsbHostConnecting);
    }
    if line.contains("USB Host disconnected...") {
        port.write_all("reset\r\n".as_bytes())?;
        return Ok(FlashState::FlashComplete);
    }
    Ok(current_state)
}

fn handle_key_event(
    port_writer: &mut Box<dyn serialport::SerialPort>,
    key_event: KeyEvent,
    char_count: &mut usize,
) -> io::Result<bool> {
    match key_event {
        KeyEvent {
            code: crossterm::event::KeyCode::Char('t'),
            modifiers: crossterm::event::KeyModifiers::CONTROL,
            ..
        } => Ok(true),

        KeyEvent {
            code: crossterm::event::KeyCode::Char('c'),
            modifiers: crossterm::event::KeyModifiers::CONTROL,
            kind: crossterm::event::KeyEventKind::Press,
            ..
        } => {
            port_writer.write_all(&[0x03])?;
            *char_count = 0;
            Ok(false)
        }

        KeyEvent {
            code,
            kind: crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat,
            ..
        } => {
            match code {
                crossterm::event::KeyCode::Char(c) => {
                    port_writer.write_all(&[c as u8])?;
                    *char_count += 1;
                }
                crossterm::event::KeyCode::Enter => {
                    port_writer.write_all(&[0x0d])?;
                    *char_count = 0;
                }
                crossterm::event::KeyCode::Backspace => {
                    if *char_count > 0 {
                        port_writer.write_all(&[0x08])?;
                        //print!("\x08");
                        //io::stdout().flush()?;
                        *char_count -= 1;
                    }
                }
                crossterm::event::KeyCode::Esc => port_writer.write_all(&[0x1b])?,
                _ => {}
            }
            Ok(false)
        }

        _ => Ok(false),
    }
}

fn setup_serial_port(
    port_name: &str,
) -> Result<Box<dyn serialport::SerialPort>, serialport::Error> {
    serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(10))
        .open()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;

    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <port>", args[0]);
        std::process::exit(1);
    }
    let port_name = &args[1];

    let port = setup_serial_port(port_name)?;
    println!("Connected to {}. Press Ctrl-T to exit.", port_name);

    let port_reader = port.try_clone()?;
    let (rx, mode_tx) = spawn_reader_thread(port_reader);

    let mut port_writer = port;
    let mut mode = Mode::Terminal;
    let mut flash_state = FlashState::Unknown;
    let mut char_count = 0;

    // Initial status line
    update_status_line(&mode, &flash_state)?;

    loop {
        // Check for flash state updates (non-blocking)
        if let Ok(new_state) = rx.try_recv() {
            flash_state = new_state;
            //update_status_line(&mode, &flash_state)?;
        }

        if event::poll(Duration::from_millis(2))? {
            if let Event::Key(key_event) = event::read()? {
                // Handle mode toggle
                match key_event {
                    KeyEvent {
                        code: KeyCode::Char('y'),
                        modifiers: KeyModifiers::CONTROL,
                        kind: KeyEventKind::Press,
                        ..
                    } => {
                        mode = match mode {
                            Mode::Terminal => Mode::Flash,
                            Mode::Flash => Mode::Terminal,
                        };
                        mode_tx.send(mode).unwrap();
                        update_status_line(&mode, &flash_state)?;
                        continue;
                    }
                    _ => {}
                }

                if handle_key_event(&mut port_writer, key_event, &mut char_count)? {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    Ok(())
}

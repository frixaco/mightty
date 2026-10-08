//! Real Windows PTY comparison, one case per process so the PowerShell runner
//! can kill a wedged constructor/shutdown. Run tools/pty-experiment.ps1.

#[cfg(all(windows, feature = "portable-pty-experiment"))]
mod experiment {
    use mightty::{
        ghostty::{RenderState, Terminal, TerminalOptions},
        profile::LaunchSpec,
        shell::{PtyControl, PtyParts, PtyRead, PtySize},
    };
    use serde_json::{Value, json};
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    struct Session {
        terminal: Terminal,
        control: PtyControl,
        input: flume::Sender<Vec<u8>>,
        output: flume::Receiver<Result<Vec<u8>, String>>,
        reader: Option<JoinHandle<()>>,
        writer: Option<JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        reader_cancel: Arc<AtomicBool>,
        responses: Rc<RefCell<Vec<Vec<u8>>>>,
        bytes: usize,
        eof: bool,
        spawn_ms: f64,
        started: Instant,
        raw: Vec<u8>,
    }
    fn cancel(thread: &JoinHandle<()>) {
        use std::os::windows::io::AsRawHandle;
        unsafe { windows_sys::Win32::System::IO::CancelSynchronousIo(thread.as_raw_handle()) };
    }
    impl Session {
        fn spawn(launch: LaunchSpec) -> Self {
            let mut terminal = Terminal::new(TerminalOptions {
                rows: 24,
                cols: 100,
                max_scrollback: 1000,
            })
            .unwrap();
            let responses = Rc::new(RefCell::new(Vec::new()));
            let callback = Rc::clone(&responses);
            terminal
                .on_pty_write(move |bytes| callback.borrow_mut().push(bytes.to_vec()))
                .unwrap();
            let started = Instant::now();
            let parts = PtyParts::spawn(&launch, PtySize::new(24, 100)).unwrap();
            let spawn_ms = started.elapsed().as_secs_f64() * 1000.0;
            let PtyParts {
                mut input,
                mut output,
                control,
            } = parts;
            let (input_tx, input_rx) = flume::bounded::<Vec<u8>>(64);
            let (output_tx, output_rx) = flume::bounded(64);
            let stop = Arc::new(AtomicBool::new(false));
            let writer_stop = Arc::clone(&stop);
            let writer = std::thread::spawn(move || {
                while let Ok(bytes) = input_rx.recv() {
                    if writer_stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Err(e) = input.write_all_interruptible(&bytes, &writer_stop) {
                        assert!(writer_stop.load(Ordering::Acquire), "input: {e}");
                        break;
                    }
                }
            });
            let reader_stop = Arc::clone(&stop);
            let reader_cancel = Arc::new(AtomicBool::new(false));
            let cancel_reader = Arc::clone(&reader_cancel);
            let reader = std::thread::spawn(move || {
                let mut buf = [0_u8; 32 * 1024];
                loop {
                    let event = match output.read_interruptible(&mut buf, &cancel_reader) {
                        Ok(PtyRead::Data(n)) => Ok(buf[..n].to_vec()),
                        Ok(PtyRead::Eof) => break,
                        Err(e) => {
                            if !reader_stop.load(Ordering::Acquire) {
                                let _ = output_tx.try_send(Err(e.to_string()));
                            }
                            break;
                        }
                    };
                    if reader_stop.load(Ordering::Acquire) {
                        continue;
                    }
                    if output_tx.send(event).is_err() {
                        break;
                    }
                }
            });
            Self {
                terminal,
                control,
                input: input_tx,
                output: output_rx,
                reader: Some(reader),
                writer: Some(writer),
                stop,
                reader_cancel,
                responses,
                bytes: 0,
                eof: false,
                spawn_ms,
                started,
                raw: Vec::new(),
            }
        }
        fn pump(&mut self, timeout: Duration) {
            match self.output.recv_timeout(timeout) {
                Ok(Ok(bytes)) => {
                    self.bytes += bytes.len();
                    if self.raw.len() < 8 * 1024 * 1024 {
                        self.raw.extend_from_slice(&bytes);
                    }
                    self.terminal.vt_write(&bytes);
                    for reply in self.responses.borrow_mut().drain(..) {
                        self.input.send(reply).unwrap();
                    }
                }
                Ok(Err(e)) => panic!("output: {e}"),
                Err(flume::RecvTimeoutError::Disconnected) => self.eof = true,
                Err(flume::RecvTimeoutError::Timeout) => {}
            }
        }
        fn text(&mut self) -> String {
            let colors = RenderState::new()
                .unwrap()
                .observe(&self.terminal)
                .unwrap()
                .colors()
                .unwrap();
            let rows = self
                .terminal
                .diagnostic_rows(false, 40, 2 * 1024 * 1024, &colors)
                .unwrap();
            rows.rows
                .iter()
                .map(|row| {
                    row.cells
                        .iter()
                        .filter(|cell| {
                            !matches!(
                                cell.width,
                                mightty::ghostty::render::CellWidth::SpacerTail
                                    | mightty::ghostty::render::CellWidth::SpacerHead
                            )
                        })
                        .map(|cell| {
                            if cell.text.is_empty() {
                                " "
                            } else {
                                cell.text.as_str()
                            }
                        })
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        fn wait_text(&mut self, marker: &str) {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                self.pump(Duration::from_millis(10));
                if String::from_utf8_lossy(&self.raw).contains(marker)
                    || self.text().contains(marker)
                {
                    return;
                }
                assert!(
                    !self.eof && Instant::now() < deadline,
                    "missing {marker:?}; tail: {}",
                    self.text()
                );
            }
        }
        fn finish(&mut self, expected: u32) {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut closed = false;
            while !self.eof {
                self.pump(Duration::from_millis(10));
                if !closed && self.control.has_exited().unwrap() {
                    assert_eq!(self.control.exit_code().unwrap(), Some(expected));
                    self.control.shutdown().unwrap();
                    closed = true;
                }
                assert!(
                    Instant::now() < deadline,
                    "output did not reach EOF; tail={:?}",
                    self.text()
                );
            }
            if !closed {
                assert_eq!(self.control.exit_code().unwrap(), Some(expected));
            }
        }
        fn close(&mut self) -> f64 {
            let started = Instant::now();
            self.stop.store(true, Ordering::Release);
            // Free output queue capacity before ClosePseudoConsole; the reader
            // keeps draining without handing data to the UI during shutdown.
            while self.output.try_recv().is_ok() {}
            if let Some(writer) = self.writer.take() {
                let _ = self.input.try_send(Vec::new());
                while !writer.is_finished() {
                    cancel(&writer);
                    std::thread::sleep(Duration::from_millis(1));
                }
                writer.join().unwrap();
            }
            self.control.shutdown().unwrap();
            self.reader_cancel.store(true, Ordering::Release);
            if let Some(reader) = self.reader.take() {
                while !reader.is_finished() {
                    cancel(&reader);
                    std::thread::sleep(Duration::from_millis(1));
                }
                reader.join().unwrap();
            }
            started.elapsed().as_secs_f64() * 1000.0
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn pwsh(script: &str) -> LaunchSpec {
        LaunchSpec::new("pwsh.exe").with_arguments([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
    }
    fn run_case(case: &str) -> Value {
        match case {
            "cmd" | "pwsh" | "powershell" => {
                let launch = match case {
                    "cmd" => LaunchSpec::new("cmd.exe").with_arguments([
                        "/d",
                        "/c",
                        "echo FINAL-OUTPUT & exit /b 37",
                    ]),
                    "powershell" => LaunchSpec::new("powershell.exe").with_arguments([
                        "-NoLogo",
                        "-NoProfile",
                        "-Command",
                        "[Console]::WriteLine('FINAL-OUTPUT'); exit 37",
                    ]),
                    _ => pwsh("[Console]::WriteLine('FINAL-OUTPUT'); exit 37"),
                };
                let mut session = Session::spawn(launch);
                session.finish(37);
                assert!(session.text().contains("FINAL-OUTPUT"));
                json!({"spawn_ms":session.spawn_ms,"total_ms":session.started.elapsed().as_secs_f64()*1000.0,"bytes":session.bytes})
            }
            "launch" => {
                let dir =
                    std::env::temp_dir().join(format!("mightty PTY 界 {}", std::process::id()));
                std::fs::create_dir_all(&dir).unwrap();
                let script = dir.join("arguments.ps1");
                std::fs::write(&script, "param([string]$First,[string]$Second,[string]$Third)\n[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); [Console]::WriteLine('ARG1='+$First); [Console]::WriteLine('ARG2='+$Second); [Console]::WriteLine('ARG3='+$Third); [Console]::WriteLine('ENV='+$env:MIGHTTY_EXPERIMENT); [Console]::WriteLine('UNSET='+$env:MIGHTTY_PTY_BACKEND); [Console]::WriteLine('CWD='+$PWD); exit 0").unwrap();
                let mut launch = LaunchSpec::new(r"C:\Program Files\PowerShell\7\pwsh.exe")
                    .with_arguments([
                        "-NoLogo",
                        "-NoProfile",
                        "-File",
                        script.to_str().unwrap(),
                        "hello world",
                        "quote\"inside",
                        "trailing\\",
                    ])
                    .with_working_directory(&dir)
                    .with_environment([("MIGHTTY_EXPERIMENT", "value 界 with spaces")]);
                launch
                    .unset_environment
                    .insert("MIGHTTY_PTY_BACKEND".into());
                let mut session = Session::spawn(launch);
                session.finish(0);
                let text = session.text();
                for marker in [
                    "ARG1=hello world",
                    "ARG2=quote\"inside",
                    "ARG3=trailing\\",
                    "ENV=value 界 with spaces",
                    "UNSET=",
                    dir.to_str().unwrap(),
                ] {
                    assert!(text.contains(marker), "{marker:?}: {text}");
                }
                assert!(!text.contains("UNSET=portable") && !text.contains("UNSET=native"));
                session.close();
                std::fs::remove_dir_all(&dir).unwrap();
                json!({"text":text,"spawn_ms":session.spawn_ms})
            }
            "env-clear" => {
                let mut launch = LaunchSpec::new(r"C:\Windows\System32\cmd.exe")
                    .with_arguments(["/d", "/c", "echo KEPT=%MIGHTTY_EXPERIMENT% & if defined MIGHTTY_PTY_BACKEND (echo LEAK) else (echo CLEARED)"])
                    .with_environment([("MIGHTTY_EXPERIMENT", "explicit")]);
                launch.inherit_environment = false;
                let mut session = Session::spawn(launch);
                session.finish(0);
                let text = session.text();
                assert!(
                    text.contains("KEPT=explicit")
                        && text.contains("CLEARED")
                        && !text.contains("LEAK"),
                    "{text}"
                );
                json!({"text":text})
            }
            "cwd-default" => {
                let mut session = Session::spawn(pwsh("[Console]::WriteLine('CWD='+$PWD)"));
                session.finish(0);
                let text = session.text();
                let expected = std::env::current_dir().unwrap();
                assert!(
                    text.contains(expected.to_str().unwrap()),
                    "expected inherited cwd {expected:?}; got {text:?}"
                );
                json!({"text":text})
            }
            "mode-probe" => {
                let mut session = Session::spawn(pwsh(
                    "$e=[char]27; [Console]::Write(\"${e}[?1049h${e}[?2004h${e}[?1002h${e}[?1006h${e}[?2026hMODES_READY\"); $null=[Console]::ReadKey($true); [Console]::Write(\"${e}[?2026l${e}[?1002l${e}[?2004l${e}[?1049lMODES_DONE\");",
                ));
                session.wait_text("MODES_READY");
                let status = session.terminal.diagnostic_status().unwrap();
                let modes = json!({"alternate":status.alternate_buffer,"paste":status.bracketed_paste,"mouse":status.mouse_tracking,"synchronized":status.synchronized_output});
                session.input.send(b"x".to_vec()).unwrap();
                session.finish(0);
                json!({"modes":modes,"raw_vt":String::from_utf8_lossy(&session.raw),"bytes":session.bytes})
            }
            "protocol-probe" => {
                use mightty::ghostty::graphics::{PlacementIterator, PlacementLayer};
                let mut session = Session::spawn(pwsh(
                    "$e=[char]27; [Console]::Write(\"${e}[2J${e}[H${e}]7;file:///C:/PTY-experiment${e}\\${e}]8;;https://example.com${e}\\LINK${e}]8;;${e}\\ ${e}[38;2;18;52;86mCOLOR${e}[0m`r`n${e}_Ga=T,t=d,f=24,i=1,p=1,s=1,v=2,c=10,r=1,q=2;////////${e}\\PROTOCOL_READY\"); $null=[Console]::ReadKey($true)",
                ));
                session
                    .terminal
                    .enable_direct_graphics(1024 * 1024)
                    .unwrap();
                session.wait_text("PROTOCOL_READY");
                let colors = RenderState::new()
                    .unwrap()
                    .observe(&session.terminal)
                    .unwrap()
                    .colors()
                    .unwrap();
                let rows = session
                    .terminal
                    .diagnostic_rows(true, 24, 2 * 1024 * 1024, &colors)
                    .unwrap();
                let color = rows.rows[0]
                    .cells
                    .iter()
                    .find(|cell| cell.column == 5)
                    .unwrap()
                    .foreground;
                let mut images = Vec::new();
                {
                    let graphics = session.terminal.graphics().unwrap();
                    let mut iterator = PlacementIterator::new().unwrap();
                    let mut placements = graphics
                        .placements(&mut iterator, PlacementLayer::All)
                        .unwrap();
                    while let Some(placement) = placements.next() {
                        let placement = placement.placement().unwrap();
                        let data = placement.image.data().unwrap();
                        images.push(json!({"width":data.width,"height":data.height,"bytes":data.pixels.len()}));
                    }
                }
                let metrics = json!({"cwd":session.terminal.working_directory().unwrap(),"hyperlink":session.terminal.hyperlink_uri(0,0).unwrap(),"color":[color.r,color.g,color.b],"images":images,"raw_vt":String::from_utf8_lossy(&session.raw)});
                session.input.send(b"x".to_vec()).unwrap();
                session.finish(0);
                metrics
            }
            "encoded-keys" => {
                use mightty::ghostty::key::{Action, Encoder, Event, Key, Mods};
                let mut session = Session::spawn(pwsh(
                    "[Console]::TreatControlCAsInput=$true; [Console]::WriteLine('READY'); for($i=0;$i -lt 5;$i++){ $k=[Console]::ReadKey($true); [Console]::WriteLine(('KEY='+$k.Key+';MOD='+$k.Modifiers)) }",
                ));
                session.wait_text("READY");
                let mut encoder = Encoder::new().unwrap();
                let mut event = Event::new().unwrap();
                let mut bytes = Vec::new();
                for (key, mods) in [
                    (Key::ArrowUp, Mods::empty()),
                    (Key::F5, Mods::empty()),
                    (Key::A, Mods::CTRL),
                    (Key::X, Mods::ALT),
                    (Key::Tab, Mods::SHIFT),
                ] {
                    let character = match key {
                        Key::A => Some("a"),
                        Key::X => Some("x"),
                        _ => None,
                    };
                    event
                        .set_action(Action::Press)
                        .set_key(key)
                        .set_mods(mods)
                        .set_consumed_mods(Mods::empty())
                        .set_unshifted_codepoint(
                            character.and_then(|s| s.chars().next()).unwrap_or('\0'),
                        )
                        .set_utf8(character)
                        .set_composing(false);
                    encoder
                        .set_options_from_terminal(&session.terminal)
                        .encode_to_vec(&event, &mut bytes)
                        .unwrap();
                }
                eprintln!("Ghostty encoded keys: {bytes:?}");
                session.input.send(bytes).unwrap();
                session.finish(0);
                let text = session.text();
                for marker in [
                    "KEY=UpArrow;MOD=",
                    "KEY=F5;MOD=",
                    "KEY=A;MOD=Control",
                    "KEY=X;MOD=Alt",
                    "KEY=Tab;MOD=Shift",
                ] {
                    assert!(text.contains(marker), "{marker}: {text}");
                }
                json!({"text":text})
            }
            "unicode-input" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::WriteLine('READY'); for($i=0;$i -lt 4;$i++){ $k=[Console]::ReadKey($true); [Console]::WriteLine(('U='+[int]$k.KeyChar)) }",
                ));
                session.wait_text("READY");
                let mut encoder = mightty::ghostty::key::Encoder::new().unwrap();
                let bytes = encoder.encode_text(&session.terminal, "é界🦀").unwrap();
                session.input.send(bytes).unwrap();
                session.finish(0);
                let text = session.text();
                for code in "é界🦀".encode_utf16() {
                    assert!(text.contains(&format!("U={code}")), "{text}");
                }
                json!({"text":text})
            }
            "resize-storm" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::WriteLine('READY'); $null=[Console]::ReadKey($true); [Console]::WriteLine(('SIZE='+[Console]::WindowWidth+'x'+[Console]::WindowHeight))",
                ));
                session.wait_text("READY");
                let started = Instant::now();
                for n in 0..100 {
                    let size = PtySize::new(24 + (n % 10), 80 + (n % 40));
                    session.control.resize(size).unwrap();
                    session
                        .terminal
                        .resize(size.cols, size.rows, 10, 20)
                        .unwrap();
                }
                session.control.resize(PtySize::new(40, 120)).unwrap();
                session.terminal.resize(120, 40, 10, 20).unwrap();
                let resize_ms = started.elapsed().as_secs_f64() * 1000.0;
                session.input.send(b"x".to_vec()).unwrap();
                session.finish(0);
                assert!(session.text().contains("SIZE=120x40"));
                json!({"resizes":101,"elapsed_ms":resize_ms})
            }
            "invalid" => {
                for size in [PtySize::new(0, 80), PtySize::new(24, 0)] {
                    assert!(PtyParts::spawn(&pwsh("exit"), size).is_err());
                }
                for launch in [
                    LaunchSpec::new("mightty-does-not-exist.exe"),
                    pwsh("exit").with_working_directory(r"C:\mightty-no-such-directory-experiment"),
                ] {
                    assert!(
                        PtyParts::spawn(&launch, PtySize::new(24, 80)).is_err(),
                        "accepted invalid launch: {launch:?}"
                    );
                }
                json!({"checked":4})
            }
            "unicode-vt" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $e=[char]27; [Console]::Write(\"${e}[2J${e}[H${e}[31mRED${e}[0m café 界 e$([char]0x301) 🦀${e}]0;PTY experiment${e}\\${e}[3;5HCURSOR\"); Start-Sleep -Milliseconds 200",
                ));
                session.finish(0);
                let text = session.text();
                for marker in ["RED", "café", "界", "e\u{301}", "🦀", "CURSOR"] {
                    assert!(text.contains(marker), "{marker}: {text}");
                }
                let status = session.terminal.diagnostic_status().unwrap();
                json!({"text":text,"title":session.terminal.title().unwrap(),"bytes":session.bytes,"raw_vt":String::from_utf8_lossy(&session.raw),"cursor":[status.cursor_x,status.cursor_y]})
            }
            "resize" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::WriteLine('READY'); $null=[Console]::ReadKey($true); [Console]::WriteLine(('SIZE='+[Console]::WindowWidth+'x'+[Console]::WindowHeight));",
                ));
                session.wait_text("READY");
                session.control.resize(PtySize::new(40, 120)).unwrap();
                session.terminal.resize(120, 40, 10, 20).unwrap();
                assert!(session.control.resize(PtySize::new(0, 80)).is_err());
                session.input.send(b"x".to_vec()).unwrap();
                session.finish(0);
                let text = session.text();
                assert!(text.contains("SIZE=120x40"), "{text}");
                json!({"text":text})
            }
            "input" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::TreatControlCAsInput=$true; [Console]::WriteLine('READY'); for($i=0;$i -lt 3;$i++){ $k=[Console]::ReadKey($true); [Console]::WriteLine(('KEY='+$k.Key+';MOD='+$k.Modifiers+';CHAR='+[int]$k.KeyChar)) }",
                ));
                session.wait_text("READY");
                session.input.send(vec![b'a', b'\r', 3]).unwrap();
                session.finish(0);
                let text = session.text();
                for marker in ["KEY=A", "KEY=Enter", "MOD=Control;CHAR=3"] {
                    assert!(text.contains(marker), "{marker}: {text}");
                }
                json!({"text":text})
            }
            "bulk-input" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::WriteLine('READY'); $n=0; while($n -lt 65536){$k=[Console]::ReadKey($true); if($k.KeyChar -ne 'x'){throw 'bad key'}; $n++}; [Console]::WriteLine('INPUT-65536-OK')",
                ));
                session.wait_text("READY");
                let started = Instant::now();
                session.input.send(vec![b'x'; 65536]).unwrap();
                session.finish(0);
                assert!(session.text().contains("INPUT-65536-OK"));
                json!({"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"input_bytes":65536})
            }
            "bulk-output" => {
                let mut session = Session::spawn(pwsh(
                    "$block=('OUTPUT-'+('x'*70)+\"`r`n\")*1024; for($i=0;$i -lt 32;$i++){[Console]::Write($block)}; [Console]::WriteLine('OUTPUT-FINAL-32768')",
                ));
                session.finish(0);
                let text = session.text();
                assert!(
                    text.contains("OUTPUT-FINAL-32768"),
                    "bytes={}, tail={:?}",
                    session.bytes,
                    text.chars().rev().take(400).collect::<String>()
                );
                json!({"elapsed_ms":session.started.elapsed().as_secs_f64()*1000.0,"bytes":session.bytes,"source_bytes":32*1024*79})
            }
            "idle-close" | "blocked-input-close" | "blocked-output-close" => {
                let script = if case == "blocked-output-close" {
                    "[Console]::WriteLine('READY'); $b=('x'*1000)+\"`r`n\"; while($true){[Console]::Write($b)}"
                } else {
                    "[Console]::WriteLine('READY'); Start-Sleep -Seconds 120"
                };
                let mut session = Session::spawn(pwsh(script));
                session.wait_text("READY");
                if case == "blocked-input-close" {
                    session.input.send(vec![b'x'; 2 * 1024 * 1024]).unwrap();
                }
                std::thread::sleep(Duration::from_millis(200));
                let shutdown_ms = session.close();
                assert!(shutdown_ms < 1500.0, "shutdown {shutdown_ms} ms");
                json!({"shutdown_ms":shutdown_ms})
            }
            "repeat" => {
                fn handles() -> u32 {
                    let mut count = 0;
                    unsafe {
                        assert_ne!(
                            windows_sys::Win32::System::Threading::GetProcessHandleCount(
                                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                                &mut count
                            ),
                            0
                        );
                    }
                    count
                }
                let mut warmup = Session::spawn(pwsh("exit"));
                warmup.finish(0);
                warmup.close();
                drop(warmup);
                let before = handles();
                let mut spawn = Vec::new();
                let mut close = Vec::new();
                for _ in 0..12 {
                    let mut session = Session::spawn(pwsh("[Console]::WriteLine('FINAL')"));
                    session.finish(0);
                    assert!(session.text().contains("FINAL"));
                    spawn.push(session.spawn_ms);
                    close.push(session.close());
                }
                let after = handles();
                assert!(after <= before + 2, "handle growth {before} -> {after}");
                json!({"handles_before":before,"handles_after":after,"spawn_ms":spawn,"shutdown_ms":close})
            }
            "concurrent" => {
                let mut sessions = Vec::new();
                for index in 0..4 {
                    let launch=pwsh("[Console]::WriteLine('READY'); $null=[Console]::ReadKey($true); [Console]::WriteLine('FINAL-'+$env:MIGHTTY_EXPERIMENT)").with_environment([("MIGHTTY_EXPERIMENT",index.to_string())]);
                    sessions.push(Session::spawn(launch));
                }
                for session in &mut sessions {
                    session.wait_text("READY");
                    session.input.send(b"x".to_vec()).unwrap();
                }
                for (index, session) in sessions.iter_mut().enumerate() {
                    session.finish(0);
                    assert!(session.text().contains(&format!("FINAL-{index}")));
                    session.close();
                }
                json!({"sessions":4})
            }
            "child-tree-close" => {
                use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
                use windows_sys::Win32::System::Threading::{
                    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                    PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
                };
                let mut session = Session::spawn(pwsh(
                    "& pwsh.exe -NoLogo -NoProfile -NonInteractive -Command '[Console]::WriteLine((\"CHILD=\"+$PID)); Start-Sleep -Seconds 120'",
                ));
                session.wait_text("CHILD=");
                let text = session.text();
                let pid = text
                    .lines()
                    .find_map(|line| {
                        line.trim()
                            .strip_prefix("CHILD=")
                            .and_then(|s| s.trim().parse::<u32>().ok())
                    })
                    .expect("child PID");
                let raw = unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                        0,
                        pid,
                    )
                };
                assert!(!raw.is_null());
                let child = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };
                let shutdown_ms = session.close();
                let status = unsafe { WaitForSingleObject(child.as_raw_handle().cast(), 3000) };
                if status != 0 {
                    unsafe { TerminateProcess(child.as_raw_handle().cast(), 1) };
                }
                assert_eq!(
                    status, 0,
                    "attached child survived shutdown; cleaned up PID {pid}"
                );
                json!({"shutdown_ms":shutdown_ms,"child_exited":true})
            }
            "killer-contract" => {
                let mut session = Session::spawn(pwsh(
                    "[Console]::WriteLine('READY'); Start-Sleep -Seconds 120",
                ));
                session.wait_text("READY");
                let PtyControl::Portable { child, .. } = &session.control else {
                    panic!("portable-only case")
                };
                let result = child.borrow().clone_killer().kill();
                session.finish(1);
                session.close();
                assert!(
                    result.is_ok(),
                    "process terminated but clone_killer reported {result:?}"
                );
                json!({"kill_ok":true})
            }
            _ => panic!("unknown case {case}"),
        }
    }
    pub fn main() {
        let case = std::env::args().nth(1).expect("case argument");
        let backend = std::env::var("MIGHTTY_PTY_BACKEND").unwrap_or("native".into());
        let started = Instant::now();
        let metrics = run_case(&case);
        use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
        let dll_name: Vec<u16> = "conpty.dll\0".encode_utf16().collect();
        let mut filename = [0_u16; 32768];
        let module = unsafe { GetModuleHandleW(dll_name.as_ptr()) };
        let engine = if module.is_null() {
            "Windows system ConPTY".to_owned()
        } else {
            let length =
                unsafe { GetModuleFileNameW(module, filename.as_mut_ptr(), filename.len() as u32) };
            String::from_utf16_lossy(&filename[..length as usize])
        };
        println!(
            "{}",
            json!({"case":case,"backend":backend,"engine":engine,"ok":true,"duration_ms":started.elapsed().as_secs_f64()*1000.0,"metrics":metrics})
        );
    }
}

#[cfg(all(windows, feature = "portable-pty-experiment"))]
fn main() {
    experiment::main();
}
#[cfg(not(all(windows, feature = "portable-pty-experiment")))]
fn main() {
    eprintln!("Windows and --features portable-pty-experiment required");
    std::process::exit(2);
}

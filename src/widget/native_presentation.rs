//! Explicit native GPU test; never launches a shell or uses global app storage.
use super::*;
use gpui::{Application, AsyncApp, WindowBounds, WindowHandle, WindowOptions, size};

async fn raster(handle: WindowHandle<TerminalWidget>, cx: &mut AsyncApp) -> image::RgbaImage {
    let baseline = handle
        .update(cx, |_, window, _| {
            let baseline = window
                .presented_metadata()
                .and_then(|data| data.downcast::<u64>().ok())
                .map_or(0, |v| *v);
            window.refresh();
            baseline
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ready = handle
            .update(cx, |_, window, _| {
                window
                    .presented_metadata()
                    .and_then(|data| data.downcast::<u64>().ok())
                    .is_some_and(|v| *v > baseline)
            })
            .unwrap();
        if ready {
            break;
        }
        assert!(Instant::now() < deadline, "native frame not presented");
        Timer::after(Duration::from_millis(10)).await;
    }
    let (capture, _) = handle
        .update(cx, |_, window, _| window.capture_presented().unwrap())
        .unwrap();
    cx.background_executor()
        .spawn(async move {
            image::RgbaImage::from_raw(capture.width, capture.height, capture.read().unwrap())
                .unwrap()
        })
        .await
}

fn write(handle: WindowHandle<TerminalWidget>, bytes: &[u8], cx: &mut AsyncApp) {
    handle
        .update(cx, |widget, _, cx| {
            widget.apply_pty_event(PtyEvent::Output(bytes.to_vec()), cx);
            widget.publish_terminal(cx);
            cx.notify();
        })
        .unwrap();
}

fn differences(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count()
}

#[test]
#[ignore = "requires an interactive Windows GPU desktop; cargo test native_terminal_presentation -- --ignored --test-threads=1"]
fn native_terminal_presentation() {
    Application::new().run(|cx| {
        widget_init(cx);
        let fonts = [
            include_bytes!("../../fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Regular.ttf")
                .as_slice(),
            include_bytes!("../../fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Bold.ttf")
                .as_slice(),
            include_bytes!("../../fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-Italic.ttf")
                .as_slice(),
            include_bytes!("../../fonts/JetBrainsMono/JetBrainsMonoNerdFontMono-BoldItalic.ttf")
                .as_slice(),
        ]
        .into_iter()
        .map(std::borrow::Cow::Borrowed)
        .collect();
        cx.text_system().add_fonts(fonts).unwrap();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        gpui::point(px(100.), px(100.)),
                        size(px(600.), px(400.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let widget = TerminalWidget::with_pty(
                            TerminalConfig {
                                cursor_style: CursorStyle::Line,
                                cursor_blink: false,
                                // Raster checks set blink phases explicitly, without a wall-clock race.
                                blink_interval: Duration::from_secs(60),
                                theme: TerminalTheme {
                                    cursor: gpui::rgb(0x00ff00),
                                    foreground: gpui::white().into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            Arc::new(AtomicBool::new(false)),
                            None,
                            None,
                            None,
                            cx,
                        );
                        widget.focus_handle.focus(window);
                        widget
                    })
                },
            )
            .unwrap();
        handle
            .update(cx, |_, window, _| {
                let counter = std::cell::Cell::new(0u64);
                window.on_frame_prepared(move |_, _| {
                    counter.set(counter.get() + 1);
                    Arc::new(counter.get())
                });
            })
            .unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let _ = raster(handle, cx).await;
            let directory = std::env::temp_dir().join(format!(
                "mightty-presentation-raster-{}",
                crate::feedback::unix_timestamp_ms()
            ));
            std::fs::create_dir_all(&directory).unwrap();
            write(handle, b"\x1b[2J\x1b[H\x1b[?25lM\x1b[H", cx);
            let hidden = raster(handle, cx).await;
            hidden.save(directory.join("hidden.png")).unwrap();
            write(handle, b"\x1b[?25h\x1b[2 q", cx);
            let block = raster(handle, cx).await;
            block.save(directory.join("block.png")).unwrap();
            assert!(differences(&hidden, &block) > 40, "filled cursor missing");
            let (width, height) = handle
                .update(cx, |widget, window, _| {
                    (
                        f32::from(widget.cell_size.0) * window.scale_factor(),
                        f32::from(widget.cell_size.1) * window.scale_factor(),
                    )
                })
                .unwrap();
            let mut green = 0;
            let mut black_inside = 0;
            for y in 1..(height as u32).saturating_sub(1) {
                for x in 1..(width as u32).saturating_sub(1) {
                    let p = block.get_pixel(x, y);
                    green += usize::from(p[1] > 180 && p[0] < 50 && p[2] < 50);
                    black_inside += usize::from(p[0] < 50 && p[1] < 50 && p[2] < 50);
                }
            }
            assert!(
                green > 5 && black_inside > 5,
                "block cursor must contain contrasting glyph pixels"
            );
            write(handle, b"\x1b[6 q", cx);
            let bar = raster(handle, cx).await;
            handle.update(cx, |_,window,cx| { window.blur(); cx.notify(); }).unwrap();
            assert_eq!(differences(&hidden,&raster(handle,cx).await),0,"unfocused cursor painted");
            handle.update(cx, |widget,window,cx| { widget.request_focus(window); cx.notify(); }).unwrap();
            assert_eq!(differences(&bar,&raster(handle,cx).await),0);
            write(handle,b"\x1b[5 q",cx);
            handle.update(cx, |widget,_,cx| {widget.cursor_blink_phase=false;cx.notify();}).unwrap();
            assert_eq!(differences(&hidden,&raster(handle,cx).await),0,"blink off phase painted");
            handle.update(cx, |widget,_,cx| {widget.cursor_blink_phase=true;cx.notify();}).unwrap();
            assert_eq!(differences(&bar,&raster(handle,cx).await),0,"blink on shape changed");
            write(handle, b"\x1b[4 q", cx);
            let underline = raster(handle, cx).await;
            assert!(differences(&bar, &block) > 20 && differences(&bar, &underline) > 10);
            // User defaults and RIS restore the steady bar.
            write(handle, b"\x1b[0 q", cx);
            assert_eq!(differences(&bar, &raster(handle, cx).await), 0);
            write(handle, b"\x1b[?25l\x1b[3;7H", cx);
            assert_eq!(
                differences(&hidden, &raster(handle, cx).await),
                0,
                "hidden movement painted"
            );
            write(handle, b"\x1b[H", cx);
            let anchor = handle
                .update(cx, |widget, _, _| widget.input_cursor_bounds().unwrap())
                .unwrap();
            write(
                handle,
                b"\x1b[?2026h\x1b[2J\x1b[5;9Hunfinished\x1b[?25h",
                cx,
            );
            assert_eq!(
                differences(&hidden, &raster(handle, cx).await),
                0,
                "unfinished sync leaked"
            );
            handle
                .update(cx, |widget, window, cx| {
                    widget.cursor_blink_phase = false;
                    assert_eq!(widget.input_cursor_bounds().unwrap(), anchor);
                    window.blur();
                    let _ = widget.offscreen_element(window, cx);
                    assert!(
                        widget.build_feedback_capture(true).unwrap().rows[0]
                            .text
                            .starts_with('M')
                    );
                    cx.notify();
                })
                .unwrap();
            assert_eq!(
                differences(&hidden, &raster(handle, cx).await),
                0,
                "local repaint leaked sync"
            );
            write(handle, b"\x1b[H\x1b[?25l\x1b[?2026l", cx);
            assert!(differences(&hidden, &raster(handle, cx).await) > 20);
            // Wide-cell block geometry is twice the normal character width.
            handle
                .update(cx, |widget, window, _| widget.request_focus(window))
                .unwrap();
            write(handle, "\x1bc\x1b[H界\x1b[H\x1b[2 q".as_bytes(), cx);
            let wide = raster(handle, cx).await;
            wide.save(directory.join("wide.png")).unwrap();
            let x = (width * 1.5) as u32;
            assert!(
                wide.get_pixel(x, 2)[1] > 180,
                "wide cursor did not cover second cell"
            );
            // A cursor on the wide tail must paint the same glyph/rectangle as its head.
            write(handle,b"\x1b[1;2H",cx);
            assert_eq!(differences(&wide,&raster(handle,cx).await),0,"wide-tail cursor shifted");
            handle.update(cx, |widget,window,_| {
                assert!(widget.committed.as_ref().unwrap().cursor.position.unwrap().at_wide_tail);
                let capture=widget.presentation(window,false,true);
                assert_eq!(capture.state["terminal_status"]["value"]["cursor_footprint"],serde_json::json!([0,0,2]));
            }).unwrap();
            write(handle, b"\x1b[?1049h\x1b[?25lALT", cx);
            let alternate = raster(handle, cx).await;
            write(handle, b"\x1b[?1049l\x1b[?25h", cx);
            assert_eq!(
                differences(&wide, &raster(handle, cx).await),
                0,
                "alternate buffer did not restore presentation"
            );
            assert!(differences(&wide, &alternate) > 20);
            // Hollow style is a native default, not a DECSCUSR sequence.
            handle
                .update(cx, |widget, _, cx| {
                    widget
                        .terminal
                        .set_default_cursor(crate::ghostty::render::CursorShape::HollowBlock, false)
                        .unwrap();
                    widget.terminal.vt_write(b"\x1b[0 q");
                    widget.presentation_dirty = true;
                    widget.publish_terminal(cx);
                })
                .unwrap();
            let hollow = raster(handle, cx).await;
            assert!(differences(&wide, &hollow) > 30);
            // Image references stay owned by the old frame throughout a hold.
            write(handle,b"\x1bc\x1b[?25l\x1b[3;3H\x1b_Ga=T,t=d,f=24,i=1,p=1,s=1,v=1,c=2,r=2;/wAA\x1b\\",cx);
            let red_image = raster(handle,cx).await;
            write(handle,b"\x1b[?2026h\x1b_Ga=d,d=A\x1b\\\x1b_Ga=T,t=d,f=24,i=2,p=1,s=1,v=1,c=2,r=2;AAD/\x1b\\",cx);
            assert_eq!(differences(&red_image,&raster(handle,cx).await),0,"live graphics leaked");
            write(handle,b"\x1b[?2026l",cx);
            assert!(differences(&red_image,&raster(handle,cx).await)>20,"graphics publication missing");
            eprintln!(
                "Native terminal presentation raster passed: {}",
                directory.display()
            );
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
}

fn widget_init(cx: &mut App) {
    gpui_component::init(cx);
    super::init(cx);
}

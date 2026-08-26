use fshell_panes::grid::{parser::GridParser, Grid};
use fshell_panes::layout::bsp::{BspLayout, Split};
use fshell_panes::proto::codec::Frame;
use fshell_panes::proto::message::{ClientMessage, ServerMessage};
use ratatui::layout::Rect;

// ---------------------------------------------------------------------------
// 1. Grid Parsing & Terminal Escape Sequences
// ---------------------------------------------------------------------------

#[test]
fn test_multiplexer_grid_parser_sgr_styling() {
    let mut grid = Grid::new(80, 24, 1000);
    let mut parser = GridParser::new();

    // Bold green text with reset
    parser.process(&mut grid, b"\x1b[1;32mActive Session\x1b[0m\n");

    {
        let vp = grid.viewport();
        let first_cell = &vp[0].cells()[0];
        assert_eq!(first_cell.character, 'A');
        assert!(first_cell.pen.bold);
    }

    // After reset, subsequent text is plain
    parser.process(&mut grid, b"Plain Text");
    {
        let vp = grid.viewport();
        let second_line = &vp[1].cells()[0];
        assert_eq!(second_line.character, 'P');
        assert!(!second_line.pen.bold);
    }
}

#[test]
fn test_multiplexer_grid_scrollback_buffering() {
    let mut grid = Grid::new(40, 5, 200);
    let mut parser = GridParser::new();

    // Push 20 lines through a 5-row viewport
    for i in 0..20 {
        let line = format!("log_entry_{}\n", i);
        parser.process(&mut grid, line.as_bytes());
    }

    assert_eq!(grid.scrollback_len(), 16);
    let vp = grid.viewport();
    assert_eq!(vp.len(), 5);
}

// ---------------------------------------------------------------------------
// 2. Binary Space Partitioning (BSP) Layout Engine
// ---------------------------------------------------------------------------

#[test]
fn test_bsp_layout_split_and_area_calculation() {
    let mut layout = BspLayout::with_root_id(1);
    assert_eq!(layout.pane_count(), 1);

    // Split pane 1 horizontally (left/right) with 50/50 ratio
    let pane_2 = layout.split(1, Split::Horizontal, 0.5);
    assert_eq!(layout.pane_count(), 2);
    assert_eq!(pane_2, 2);

    // Split pane 2 vertically (top/bottom) with 50/50 ratio
    let pane_3 = layout.split(2, Split::Vertical, 0.5);
    assert_eq!(layout.pane_count(), 3);
    assert_eq!(pane_3, 3);

    // Calculate layout for an 80x24 terminal window
    let total_area = Rect::new(0, 0, 80, 24);
    let rects = layout.compute_layout(total_area);
    assert_eq!(rects.len(), 3);

    // Pane 1 should occupy left half: x=0, width=40, height=24
    let (_, r1) = rects.iter().find(|(id, _)| *id == 1).unwrap();
    assert_eq!(r1.x, 0);
    assert_eq!(r1.width, 40);
    assert_eq!(r1.height, 24);

    // Close pane 2, verifying tree re-balancing
    layout.remove(2);
    assert_eq!(layout.pane_count(), 2);
    let updated_rects = layout.compute_layout(total_area);
    assert_eq!(updated_rects.len(), 2);
}

// ---------------------------------------------------------------------------
// 3. IPC Message Serialization & Codec Framing
// ---------------------------------------------------------------------------

#[test]
fn test_ipc_client_and_daemon_message_roundtrip() {
    // Client -> Daemon Attach message
    let client_msg = ClientMessage::Attach {
        session_name: "dev_session".to_string(),
        cols: 120,
        rows: 40,
    };
    let frame = Frame::from_client(&client_msg);
    let decoded_client = frame.into_client().unwrap();
    assert_eq!(client_msg, decoded_client);

    // Server -> Client ExitClient message
    let server_msg = ServerMessage::ExitClient;
    let server_frame = Frame::from_server(&server_msg);
    let decoded_server = server_frame.into_server().unwrap();
    assert_eq!(server_msg, decoded_server);
}

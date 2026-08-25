#![no_main]

use fshell_panes::grid::Grid;
use fshell_panes::grid::parser::GridParser;
use fshell_panes::grid::reflow::Reflow;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 8192 {
        return;
    }

    let mut grid = Grid::new(80, 24, 100);
    let mut parser = GridParser::new();
    let _ = parser.process(&mut grid, data);

    // Also fuzz reflow with variable column width
    if !data.is_empty() {
        let new_cols = ((data[0] as usize) % 120).max(10);
        let mut reflow = Reflow::new(&mut grid);
        reflow.reflow(new_cols);
    }
});

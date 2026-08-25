// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

pub mod eval_builtin;
pub mod getopts;
pub mod printf;
pub mod read_cmd;
pub mod shift;
pub mod test_builtin;
pub mod type_cmd;

pub use eval_builtin::eval_posix;
pub use getopts::getopts_posix;
pub use printf::printf_posix;
pub use read_cmd::read_posix;
pub use shift::{set_posix, shift_posix};
pub use test_builtin::eval_test_expr;
pub use type_cmd::type_posix;

use chrono::prelude::*;

pub(super) const TIMEOUT_SECOND: u64 = 3 * 60;

pub(super) fn timestamp() -> i64 {
	let dt = Local::now();
	dt.timestamp()
}

pub(super) fn is_timed_out(time: i64, timeout: u64) -> bool {
	let cur = timestamp();
	cur >= time + timeout as i64
}

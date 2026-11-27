use crate::stats::{Rate, Tally, Window, percent};

pub fn summarize(lines: &[(u64, usize, u32)]) -> String {
    let Some(&(start, _, _)) = lines.first() else {
        return String::from("no lines");
    };
    let mut tally = Tally::new(4);
    let mut window = Window::new(16);
    let mut rate = Rate::new(start);
    for &(second, level, latency) in lines {
        tally.record(level);
        window.push(latency);
        rate.observe(second);
    }
    let busiest = tally.busiest();
    let share = percent(tally.count(busiest), tally.total());
    format!(
        "level {busiest} is {share}% of lines; latency {} ms mean, {} ms worst; {} lines a second",
        window.mean(),
        window.worst(),
        rate.per_second()
    )
}

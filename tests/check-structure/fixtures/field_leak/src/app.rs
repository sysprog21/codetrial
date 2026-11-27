use crate::{queue, sched};

pub fn run(tasks: &[u32]) -> usize {
    let mut ready = queue::fresh(4);
    let mut ran = 0;
    for &task in tasks {
        if !sched::admit(&mut ready, task) {
            ran += drain(&mut ready);
        }
    }
    ran + sched::backlog(&ready)
}

fn drain(ready: &mut queue::RunQueue) -> usize {
    let mut ran = 0;
    while !sched::drained(ready) {
        if sched::next(ready).is_some() {
            ran += 1;
        }
    }
    ran
}

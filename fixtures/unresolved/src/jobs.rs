pub trait Job {
    fn run(&self) -> u32;
}

pub struct Email;
pub struct Report;

impl Job for Email {
    fn run(&self) -> u32 {
        1
    }
}

impl Job for Report {
    fn run(&self) -> u32 {
        2
    }
}

pub fn run_all(jobs: &[Box<dyn Job>]) -> u32 {
    let mut total = 0;
    for job in jobs {
        total += job.run();
    }
    total
}

pub fn run_generic<J: Job>(job: J) -> u32 {
    job.run()
}

pub fn drop_it(email: Email) {
    std::mem::drop(email);
}

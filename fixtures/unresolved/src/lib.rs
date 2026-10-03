pub mod jobs;
pub mod util;

macro_rules! make_helper {
    () => {
        pub fn helper_from_macro() -> u32 {
            7
        }
    };
}

make_helper!();

pub fn uses_macro_item() -> u32 {
    helper_from_macro()
}

pub fn uses_closures() -> u32 {
    let add_one = |x: u32| x + 1;
    let pick = || add_one;
    add_one(1) + (pick())(2)
}

#[cfg(feature = "legacy")]
mod legacy {
    pub fn run() {
        normalize("x");
        crate::util::missing();
    }
}

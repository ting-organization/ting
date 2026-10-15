//! Independent, monotonic resource budgets (wiki: resources/budgets).
//!
//! Every debit is charged to the running frame and all of its ancestors at
//! once. Rather than walking the ancestors on each debit, the meter keeps one
//! running total and, per frame, the total at which the frame opened and the
//! smallest ceiling (budget plus opening total) along its chain; a debit fits
//! exactly when it fits under the top frame's ceiling. Check, charge and the
//! headroom are then constant-time, whatever the nesting depth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resources(pub [u64; 4]);

impl Resources {
    pub const ZERO: Self = Self([0; 4]);
    pub fn compute(n: u64) -> Self {
        Self([n, 0, 0, 0])
    }
    pub fn state(n: u64) -> Self {
        Self([0, n, 0, 0])
    }
    pub fn data(n: u64) -> Self {
        Self([0, 0, n, 0])
    }
    pub fn growth(n: u64) -> Self {
        Self([0, 0, 0, n])
    }
    pub fn fits(self, bound: Self) -> bool {
        (0..4).all(|i| self.0[i] <= bound.0[i])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exhausted(pub usize);

#[derive(Clone, Debug)]
struct Frame {
    budget: Resources,
    /// The running total when the frame opened; its own use is the total since.
    opened: Resources,
    /// `budget + opened`: the running total may not pass it. A pushed budget
    /// fits what its parent has left, so this is never above the parent's.
    ceiling: Resources,
}

#[derive(Clone, Debug)]
pub struct Meter {
    frames: Vec<Frame>,
    /// Everything charged since the root frame opened.
    charged: Resources,
}

impl Meter {
    pub fn new(budget: Resources) -> Self {
        Self {
            frames: vec![Frame {
                budget,
                opened: Resources::ZERO,
                ceiling: budget,
            }],
            charged: Resources::ZERO,
        }
    }
    /// The root frame's consumption.
    pub fn used(&self) -> Resources {
        self.charged
    }
    /// The top frame's budget minus its consumption.
    pub fn remaining(&self) -> Resources {
        let f = self.frames.last().expect("root frame exists");
        Resources(std::array::from_fn(|i| {
            f.budget.0[i] - (self.charged.0[i] - f.opened.0[i])
        }))
    }
    /// The largest debit of one dimension every frame still accepts.
    pub fn headroom(&self, dimension: usize) -> u64 {
        let f = self.frames.last().expect("root frame exists");
        f.ceiling.0[dimension] - self.charged.0[dimension]
    }
    pub fn check(&self, debit: Resources) -> Result<(), Exhausted> {
        // Check dimensions in order: C/S/D/G precedence is protocol-defined.
        let f = self.frames.last().expect("root frame exists");
        for i in 0..4 {
            if debit.0[i] > f.ceiling.0[i] - self.charged.0[i] {
                return Err(Exhausted(i));
            }
        }
        Ok(())
    }
    pub fn charge(&mut self, debit: Resources) -> Result<(), Exhausted> {
        self.check(debit)?;
        for i in 0..4 {
            self.charged.0[i] += debit.0[i];
        }
        Ok(())
    }
    pub fn push(&mut self, budget: Resources) -> bool {
        if !budget.fits(self.remaining()) {
            return false;
        }
        let ceiling = Resources(std::array::from_fn(|i| {
            budget.0[i].saturating_add(self.charged.0[i])
        }));
        self.frames.push(Frame {
            budget,
            opened: self.charged,
            ceiling,
        });
        true
    }
    /// Closes the top frame and returns what it consumed.
    pub fn pop(&mut self) -> Resources {
        assert!(self.frames.len() > 1);
        let f = self.frames.pop().unwrap();
        Resources(std::array::from_fn(|i| self.charged.0[i] - f.opened.0[i]))
    }
}

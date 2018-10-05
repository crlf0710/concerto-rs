extern crate fixedbitset;
extern crate slab;
extern crate smallvec;
extern crate vec_drain_where;
#[macro_use]
extern crate log;

use std::fmt::Debug;

pub trait ActionConfiguration: 'static {
    type Target: Clone + PartialEq + Debug;
    type KeyKind: Clone + PartialEq + Ord + Debug;
    type CursorPos: Clone + PartialEq;

    type Command: Clone;
}

mod context;
mod execution;
mod recipe;

pub use context::*;
pub use recipe::*;

/*


enum TargetOrFilter<T> {
    Target(T),
    TargetFilter(Box<Fn(T) -> bool>),
}

pub struct ActionRecipeBuilder<'a, T: ActionConfiguration> {
    context: &'a mut ActionContext<T>,
    conditions: Vec<ActionRecipeCondition<T>>,
}

pub impl ActionContext<C> where C: ActionConfiguration {
    pub fn add_action_recipe(&mut self) -> ActionRecipeBuilder<T> {
        ActionRecipeBuilder {
            context: self,
            conditions: vec![],
    }
}

impl<'a> ActionRecipeBuilder<'a, T> {
    pub fn add_condition(&mut self, condition: ActionRecipeCondition<T>) {
        self.conditions.push(condition);
    }


    pub fn with_target(&mut self, target: T) -> &mut Self
        where T: PartialEq
    {
        self.target = Some(TargetOrFilter::Target(target));
        self
    }

    pub fn with_target_filter<F>(target_filter: F) -> &mut Self
        where F: Fn(T) -> bool
    {
        self.target = Some(TargetOrFilter::TargetFilter(Box::new(target_filter) as _));
        self
    }
}

impl<'a> ActionRecipeBuilder<'a, T> where T: KeyStrokeActionConfiguration + CursorActionConfiguration{

    pub fn trigger_by_left_button_click(&mut self) -> &mut Self {

    }

    pub fn trigger_by_right_button_down(&mut self) -> &mut Self {

    }

    pub fn trigger_by_left_and_right_button_click(&mut self) -> &mut Self {

    }

}



pub trait TargetResolver {
    type Target;

    fn resolve_target(&self, point: Point) -> Self::Target;
}


*/

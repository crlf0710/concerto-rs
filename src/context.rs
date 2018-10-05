use execution::{ActionExecutionCtx, ExecutionContextResult};
use recipe::ActionInput;
use recipe::ActionRecipeBuilder;
use recipe::{ActionRecipe, ActionRecipeItem};
use slab::Slab;
use std::collections::BTreeSet;
use ActionConfiguration;

pub struct ActionContext<C: ActionConfiguration> {
    recipe_items: ActionRecipeItemStore<C>,
    recipes: Vec<(ActionRecipe<C>, Option<ActionExecutionCtx<C>>)>,
    command_list: Vec<C::Command>,
    env_tracking_state: ActionEnvironmentTrackingState<C>,
}

pub(crate) struct ActionEnvironmentTrackingState<C: ActionConfiguration> {
    pressed_keys: BTreeSet<C::KeyKind>,
}

impl<C: ActionConfiguration> ActionEnvironmentTrackingState<C> {
    fn new() -> Self {
        ActionEnvironmentTrackingState {
            pressed_keys: BTreeSet::new(),
        }
    }

    fn update_with_input(&mut self, input: &ActionInput<C>) {
        match input {
            ActionInput::KeyDown(c) => {
                self.pressed_keys.insert(c.clone());
            }
            ActionInput::KeyUp(c) => {
                self.pressed_keys.remove(c);
            }
            _ => {}
        }
    }

    pub(crate) fn is_key_pressed(&self, key: &C::KeyKind) -> bool {
        self.pressed_keys.contains(key)
    }
}

pub(crate) struct ActionRecipeItemStore<C: ActionConfiguration>(Slab<ActionRecipeItem<C>>);

impl<C: ActionConfiguration> ActionRecipeItemStore<C> {
    fn new() -> Self {
        ActionRecipeItemStore(Slab::new())
    }

    pub(crate) fn register_item(&mut self, item: ActionRecipeItem<C>) -> ActionRecipeItemIdx {
        ActionRecipeItemIdx(self.0.insert(item))
    }

    pub(crate) fn get(&self, idx: ActionRecipeItemIdx) -> &ActionRecipeItem<C> {
        self.0
            .get(idx.0)
            .expect("ActionRecipeItemStore out-of-bound access!")
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct ActionRecipeItemIdx(usize);

impl<C: ActionConfiguration> ActionContext<C> {
    pub fn process_inputs(&mut self, inputs: &[ActionInput<C>]) -> bool {
        let mut result = false;
        for input in inputs {
            if self.process_input(input) {
                result = true;
            }
        }
        result
    }

    pub fn process_input(&mut self, input: &ActionInput<C>) -> bool {
        use std::mem::drop;
        use vec_drain_where::VecDrainWhereExt;

        self.env_tracking_state.update_with_input(input);

        let mut input_used = false;
        let mut some_recipe_finished = false;
        let mut some_effect_occurred = false;
        //first, let's see if we can procede with existing half-baked recipes.
        let recipe_items = &self.recipe_items;
        let command_list = &mut self.command_list;
        let env_tracking_state = &self.env_tracking_state;
        'step_1: for (recipe, exec_ctx) in self.recipes.iter_mut() {
            let mut remove_exec_ctx = false;

            if let Some(exec_ctx) = exec_ctx {
                match exec_ctx.process_input(
                    input,
                    recipe_items,
                    recipe,
                    command_list,
                    env_tracking_state,
                ) {
                    ExecutionContextResult::Done => {
                        input_used = true;
                        some_recipe_finished = true;
                        remove_exec_ctx = true;
                    }
                    ExecutionContextResult::Used => {
                        input_used = true;
                        some_effect_occurred = true;
                        remove_exec_ctx = false;
                    }
                    ExecutionContextResult::Ignore => {
                        remove_exec_ctx = false;
                    },
                    ExecutionContextResult::Abort => {
                        remove_exec_ctx = true;
                    },
                };
            }

            if remove_exec_ctx {
                if let Some(exec_ctx) = exec_ctx {
                    if exec_ctx.clear_effects(command_list) {
                        some_effect_occurred = true;
                    }
                }
                *exec_ctx = None;
            }
        }

        if some_recipe_finished {
            debug!(target: "concerto", "finished one recipe, clear all executions.");
            for (recipe, exec_ctx) in self.recipes.iter_mut() {
                if let Some(exec_ctx) = exec_ctx {
                    if exec_ctx.clear_effects(command_list) {
                        some_effect_occurred = true;
                    }
                }
                *exec_ctx = None;
            }
            return true;
        }

        //second, let's see if we can start new recipe with this input
        let mut rebuild_recipe_counter = 0;
        'step_2: for (recipe_idx, (recipe, exec_ctx)) in self.recipes.iter_mut().enumerate() {
            if exec_ctx.is_none() {
                let (result, new_exec_ctx) =
                    ActionExecutionCtx::start_execution_with_input(
                        input,
                        &self.recipe_items,
                        recipe,
                        recipe_idx,
                        command_list,
                        &self.env_tracking_state,
                    );

                match result {
                    ExecutionContextResult::Done => {
                        assert!(new_exec_ctx.is_none());

                        some_recipe_finished = true;
                        break 'step_2;
                    }
                    ExecutionContextResult::Used => {
                        assert!(new_exec_ctx.is_some());
                        *exec_ctx = new_exec_ctx;
                        some_effect_occurred = true;
                        rebuild_recipe_counter += 1;
                    }
                    _ => {
                        assert!(new_exec_ctx.is_none());
                    }
                }

            }
        }

        if some_recipe_finished {
            debug!(target: "concerto", "immediately finished one recipe, clear all executions.");
            for (recipe, exec_ctx) in self.recipes.iter_mut() {
                if let Some(exec_ctx) = exec_ctx {
                    if exec_ctx.clear_effects(command_list) {
                        some_effect_occurred = true;
                    }
                }
                *exec_ctx = None;
            }
            return true;
        }

        if rebuild_recipe_counter > 0 {
            debug!(target: "concerto", "rebuild {} recipes.", rebuild_recipe_counter);
        }
        some_effect_occurred
    }

    pub fn collect_commands(&mut self) -> Option<impl Iterator<Item = C::Command> + '_> {
        if self.command_list.is_empty() {
            None
        } else {
            Some(self.command_list.drain(..))
        }
    }
}

pub struct ActionContextBuilder<C: ActionConfiguration> {
    pub(crate) recipe_items: ActionRecipeItemStore<C>,
    recipes: Vec<ActionRecipe<C>>,
}

impl<C: ActionConfiguration> ActionContextBuilder<C> {
    pub fn new() -> Self {
        ActionContextBuilder {
            recipe_items: ActionRecipeItemStore::new(),
            recipes: Vec::new(),
        }
    }

    pub fn build(self) -> ActionContext<C> {
        ActionContext {
            recipe_items: self.recipe_items,
            recipes: self.recipes.into_iter().map(|x| (x, None)).collect(),
            command_list: Vec::new(),
            env_tracking_state: ActionEnvironmentTrackingState::new(),
        }
    }
}

impl<C: ActionConfiguration> ActionContextBuilder<C> {
    pub fn add_recipe<F>(mut self, f: F) -> Self
    where
        F: FnOnce(ActionRecipeBuilder<C>) -> ActionRecipe<C>,
    {
        let recipe = {
            let builder = ActionRecipeBuilder::new(&mut self);

            (f)(builder)
        };

        self.recipes.push(recipe);
        self
    }
}

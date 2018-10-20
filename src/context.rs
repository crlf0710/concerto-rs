use execution::{ActionExecutionCtx, ExecutionContextResult};
use recipe::ActionInput;
use recipe::ActionNestRecipeCommand;
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
    fn locate_nest_recipe(
        recipes: &Vec<(ActionRecipe<C>, Option<ActionExecutionCtx<C>>)>,
        recipe_idx: usize,
        nest_recipe_idx: usize,
    ) -> Option<usize> {
        if let Some((recipe, _)) = recipes.get(recipe_idx) {
            recipe.nest_recipes.get(nest_recipe_idx).cloned()
        } else {
            None
        }
    }

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
        //use std::mem::drop;
        debug!(target: "concerto", "process_input {:?}.", input);
        self.env_tracking_state.update_with_input(input);

        let mut some_recipe_finished = false;
        let mut some_effect_occurred = false;
        //first, let's see if we can procede with existing half-baked recipes.
        let recipe_items = &self.recipe_items;
        let command_list = &mut self.command_list;
        let env_tracking_state = &self.env_tracking_state;
        let mut temporary_nest_recipe_command_list = &mut Vec::new();
        'step_1: for (recipe, exec_ctx) in self.recipes.iter_mut() {
            let mut remove_exec_ctx = false;
            if let Some(exec_ctx) = exec_ctx {
                match exec_ctx.process_input(
                    input,
                    recipe_items,
                    recipe,
                    command_list,
                    &mut temporary_nest_recipe_command_list,
                    env_tracking_state,
                ) {
                    ExecutionContextResult::Done => {
                        some_recipe_finished = true;
                        remove_exec_ctx = true;
                    }
                    ExecutionContextResult::Used => {
                        some_effect_occurred = true;
                        remove_exec_ctx = false;
                    }
                    ExecutionContextResult::Ignore => {
                        remove_exec_ctx = false;
                    }
                    ExecutionContextResult::Abort => {
                        remove_exec_ctx = true;
                    }
                };
            }

            if remove_exec_ctx {
                if let Some(exec_ctx) = exec_ctx {
                    if exec_ctx.clean_up(command_list, &mut temporary_nest_recipe_command_list) {
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
                    if exec_ctx.clean_up(command_list, temporary_nest_recipe_command_list) {
                        some_effect_occurred = true;
                    }
                }
                *exec_ctx = None;
                recipe.is_enabled = !recipe.is_nested;
            }
            return true;
        }

        //second, let's see if we can start new recipe with this input
        let mut rebuild_recipe_counter = 0;
        'step_2: for (recipe_idx, (recipe, exec_ctx)) in self.recipes.iter_mut().enumerate() {
            if !recipe.is_enabled {
                continue;
            }
            if exec_ctx.is_some() {
                continue;
            }
            let (result, new_exec_ctx) = ActionExecutionCtx::start_execution_with_input(
                input,
                &self.recipe_items,
                recipe,
                recipe_idx,
                command_list,
                &mut temporary_nest_recipe_command_list,
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

        if some_recipe_finished {
            debug!(target: "concerto", "immediately finished one recipe, clear all executions.");
            for (recipe, exec_ctx) in self.recipes.iter_mut() {
                if let Some(exec_ctx) = exec_ctx {
                    if exec_ctx.clean_up(command_list, temporary_nest_recipe_command_list) {
                        some_effect_occurred = true;
                    }
                }
                *exec_ctx = None;
                recipe.is_enabled = !recipe.is_nested;
            }
            return true;
        }

        if rebuild_recipe_counter > 0 {
            debug!(target: "concerto", "rebuild {} recipes.", rebuild_recipe_counter);
        }

        while !temporary_nest_recipe_command_list.is_empty() {
            let mut new_nest_recipe_command_list = Vec::new();
            for nest_recipe_cmd in temporary_nest_recipe_command_list.drain(..) {
                match nest_recipe_cmd {
                    ActionNestRecipeCommand::Enable(recipe_idx, nest_recipe_idx) => {
                        if let Some(real_recipe_idx) =
                            Self::locate_nest_recipe(&self.recipes, recipe_idx, nest_recipe_idx)
                        {
                            debug!(target: "concerto", "nest recipe {} is now enabled.", rebuild_recipe_counter);
                            self.recipes[real_recipe_idx].0.is_enabled = true;
                        }
                    }
                    ActionNestRecipeCommand::Disable(recipe_idx, nest_recipe_idx) => {
                        if let Some(real_recipe_idx) =
                            Self::locate_nest_recipe(&self.recipes, recipe_idx, nest_recipe_idx)
                        {
                            self.recipes[real_recipe_idx].0.is_enabled = false;
                        }
                    }
                    ActionNestRecipeCommand::Abort(recipe_idx, nest_recipe_idx) => {
                        if let Some(real_recipe_idx) =
                            Self::locate_nest_recipe(&self.recipes, recipe_idx, nest_recipe_idx)
                        {
                            self.recipes[real_recipe_idx].0.is_enabled = false;

                            if let Some(exec_ctx) = &mut self.recipes[real_recipe_idx].1 {
                                if exec_ctx
                                    .clean_up(command_list, &mut new_nest_recipe_command_list)
                                {
                                    some_effect_occurred = true;
                                }
                            }
                            self.recipes[real_recipe_idx].1 = None;
                        }
                    }
                }
            }
            temporary_nest_recipe_command_list.extend(new_nest_recipe_command_list.into_iter());
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
    pub(crate) fn register_nested_recipe(&mut self, mut nest_recipe: ActionRecipe<C>) -> usize {
        nest_recipe.is_nested = true;
        nest_recipe.is_enabled = false;
        let allocated_idx = self.recipes.len();
        self.recipes.push(nest_recipe);
        allocated_idx
    }

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

use context::ActionEnvironmentTrackingState;
use context::ActionRecipeItemIdx;
use context::ActionRecipeItemStore;
use fixedbitset::FixedBitSet;
use recipe::ActionNestRecipeCommand;
use recipe::{ActionCondition, ActionInput};
use recipe::{ActionRecipe, ActionRecipeItem};
use smallvec::SmallVec;
use ActionConfiguration;

enum ActionExecutionFrame {
    Sequential(Option<usize>),
    Unordered(FixedBitSet),
    Choice(Option<usize>),
}

use std::collections::BTreeMap;

pub(crate) struct ActionExecutionCtx<C: ActionConfiguration> {
    recipe_idx: usize,
    backtrace: SmallVec<[(ActionRecipeItemIdx, ActionExecutionFrame); 3]>,
    stored_contracts: ActionExecutionContractStore<C>,
}

enum ActionExecutionContract<C: ActionConfiguration> {
    Input(ActionInput<C>),
    Condition(ActionCondition<C>),
    Effect(C::Command),
    NestRecipe(usize),
    NestRecipeDisable(usize),
}

struct ActionExecutionContractStore<C: ActionConfiguration> {
    contracts: BTreeMap<ActionRecipeItemIdx, ActionExecutionContract<C>>,
}

impl<C: ActionConfiguration> ActionExecutionContractStore<C> {
    pub(crate) fn new() -> Self {
        ActionExecutionContractStore {
            contracts: BTreeMap::new(),
        }
    }

    pub(crate) fn add_input(&mut self, item: ActionRecipeItemIdx, input_contract: ActionInput<C>) {
        self.contracts
            .insert(item, ActionExecutionContract::Input(input_contract));
    }

    pub(crate) fn add_condition(
        &mut self,
        item: ActionRecipeItemIdx,
        condition_contract: ActionCondition<C>,
    ) {
        self.contracts
            .insert(item, ActionExecutionContract::Condition(condition_contract));
    }

    pub(crate) fn add_effect(
        &mut self,
        item: ActionRecipeItemIdx,
        effect_end_contract: C::Command,
    ) {
        self.contracts
            .insert(item, ActionExecutionContract::Effect(effect_end_contract));
    }

    pub(crate) fn add_nest_recipe(&mut self, item: ActionRecipeItemIdx, nest_recipe: usize) {
        self.contracts
            .insert(item, ActionExecutionContract::NestRecipe(nest_recipe));
    }

    pub(crate) fn add_nest_recipe_disabled(
        &mut self,
        item: ActionRecipeItemIdx,
        nest_recipe: usize,
    ) {
        self.contracts.insert(
            item,
            ActionExecutionContract::NestRecipeDisable(nest_recipe),
        );
    }

    fn internal_eliminate_contract(
        &mut self,
        recipe_id: usize,
        contract: ActionExecutionContract<C>,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
    ) -> bool {
        match contract {
            ActionExecutionContract::Effect(effect_end) => {
                command_list.push(effect_end);
                true
            }
            ActionExecutionContract::NestRecipe(id) => {
                nest_recipe_command_list.push(ActionNestRecipeCommand::Abort(recipe_id, id));
                false
            }
            ActionExecutionContract::NestRecipeDisable(id) => {
                nest_recipe_command_list.push(ActionNestRecipeCommand::Enable(recipe_id, id));
                false
            }
            _ => false,
        }
    }

    pub(crate) fn eliminate(
        &mut self,
        recipe_id: usize,
        item: &ActionRecipeItemIdx,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
    ) -> bool {
        if let Some(contract) = self.contracts.remove(item) {
            self.internal_eliminate_contract(
                recipe_id,
                contract,
                command_list,
                nest_recipe_command_list,
            )
        } else {
            false
        }
    }

    pub(crate) fn eliminate_all(
        &mut self,
        recipe_id: usize,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
    ) -> bool {
        use std::mem::swap;

        let mut contracts = BTreeMap::new();
        swap(&mut self.contracts, &mut contracts);
        let mut new_command = false;
        for (k, contract) in contracts {
            if self.internal_eliminate_contract(
                recipe_id,
                contract,
                command_list,
                nest_recipe_command_list,
            ) {
                new_command = true;
            }
        }
        new_command
    }
}

pub struct ActionRecipeExecutionInfo<'a, C: ActionConfiguration> {
    stored_contracts: &'a ActionExecutionContractStore<C>,
}

impl<'a, C: ActionConfiguration> ActionRecipeExecutionInfo<'a, C> {
    fn new(stored_contracts: &'a ActionExecutionContractStore<C>) -> Self {
        ActionRecipeExecutionInfo { stored_contracts }
    }
    pub fn cursor_coordinate(&self) -> Option<&C::Target> {
        for (idx, contract) in self.stored_contracts.contracts.iter() {
            match contract {
                ActionExecutionContract::Input(expected_input) => match expected_input {
                    ActionInput::CursorCoordinate(target) => return Some(target),
                    _ => {}
                },
                _ => {}
            }
        }
        None
    }
}

impl<C: ActionConfiguration> ActionExecutionCtx<C> {
    fn new(
        recipe_idx: usize,
        recipe: &ActionRecipe<C>,
        recipe_items: &ActionRecipeItemStore<C>,
    ) -> Self {
        let mut ctx = ActionExecutionCtx {
            recipe_idx,
            backtrace: SmallVec::new(),
            stored_contracts: ActionExecutionContractStore::new(),
        };

        ctx.backtrace
            .push(Self::prepare_new_frame_for_compound_item(
                recipe_items.get(recipe.root_item),
                recipe.root_item,
            ));
        ctx
    }
    pub(crate) fn get_recipe<'a>(&self, recipes: &'a [ActionRecipe<C>]) -> &'a ActionRecipe<C> {
        &recipes[self.recipe_idx]
    }

    fn stored_contracts_conflict(
        input: &ActionInput<C>,
        stored_contracts: &ActionExecutionContractStore<C>,
    ) -> bool {
        for (idx, contract) in stored_contracts.contracts.iter() {
            match contract {
                ActionExecutionContract::Input(expected_input) => {
                    match Self::check_input_match_input(expected_input, input) {
                        ExecutionContextResult::Abort => return true,
                        _ => {}
                    }
                }
                ActionExecutionContract::Condition(condition) => {
                    match Self::check_input_match_condition(condition, input) {
                        ExecutionContextResult::Abort => return true,
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        false
    }

    pub(crate) fn make_input_filter_with_cursor_coordinate_filter<F>(
        filter: F,
    ) -> impl Fn(&ActionInput<C>) -> ExecutionContextResult
    where
        F: Fn(&C::Target) -> bool + 'static,
    {
        move |input: &ActionInput<C>| match input {
            ActionInput::CursorCoordinate(target) => {
                if (filter)(target) {
                    ExecutionContextResult::Used
                } else {
                    ExecutionContextResult::Abort
                }
            }
            _ => ExecutionContextResult::Ignore,
        }
    }

    fn check_input_match_input(
        expected_input: &ActionInput<C>,
        input: &ActionInput<C>,
    ) -> ExecutionContextResult {
        match (expected_input, input) {
            (ActionInput::CursorCoordinate(v1), ActionInput::CursorCoordinate(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Used
                } else {
                    ExecutionContextResult::Abort
                }
            }
            (ActionInput::CursorCoordinate(v1), _) => ExecutionContextResult::Ignore,
            (ActionInput::FocusCoordinate(v1), ActionInput::FocusCoordinate(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Used
                } else {
                    ExecutionContextResult::Abort
                }
            }
            (ActionInput::FocusCoordinate(v1), _) => ExecutionContextResult::Ignore,
            (ActionInput::KeyDown(v1), ActionInput::KeyDown(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Used
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            (ActionInput::KeyDown(v1), ActionInput::KeyUp(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Abort
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            (ActionInput::KeyDown(_v1), _) => ExecutionContextResult::Ignore,
            (ActionInput::KeyUp(v1), ActionInput::KeyUp(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Used
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            (ActionInput::KeyUp(v1), ActionInput::KeyDown(v2)) => {
                if v1 == v2 {
                    ExecutionContextResult::Abort
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            (ActionInput::KeyUp(_v1), _) => ExecutionContextResult::Ignore,
        }
    }

    fn check_input_match_condition(
        condition: &ActionCondition<C>,
        input: &ActionInput<C>,
    ) -> ExecutionContextResult {
        match (condition, input) {
            (ActionCondition::KeyPressed(b_k, false), ActionInput::KeyDown(k)) => {
                if b_k == k {
                    ExecutionContextResult::Abort
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            (ActionCondition::KeyPressed(b_k, true), ActionInput::KeyUp(k)) => {
                if b_k == k {
                    ExecutionContextResult::Abort
                } else {
                    ExecutionContextResult::Ignore
                }
            }
            _ => ExecutionContextResult::Ignore,
        }
    }

    fn check_interactive_item_match_input(
        item: &ActionRecipeItem<C>,
        input: &ActionInput<C>,
    ) -> ExecutionContextResult {
        debug_assert!(item.is_interactive());
        match item {
            ActionRecipeItem::StartInput(expected_input) => {
                Self::check_input_match_input(expected_input, input)
            }
            ActionRecipeItem::StartFilteredInput(filter) => (filter)(&input),
            _ => {
                unreachable!();
            }
        }
    }

    fn check_condition_match_environment(
        condition_item: &ActionCondition<C>,
        env: &ActionEnvironmentTrackingState<C>,
    ) -> bool {
        match condition_item {
            ActionCondition::KeyPressed(k, s) => {
                if env.is_key_pressed(k) != *s {
                    return false;
                }
            }
        }
        true
    }

    fn check_condition_item_match_environment(
        recipe_item_idx: ActionRecipeItemIdx,
        recipe_item: &ActionRecipeItem<C>,
        stored_contracts: &mut ActionExecutionContractStore<C>,
        env: &ActionEnvironmentTrackingState<C>,
    ) -> ExecutionContextResult {
        debug_assert!(recipe_item.is_condition());
        match recipe_item {
            ActionRecipeItem::StartCondition(condition) => {
                if !Self::check_condition_match_environment(condition, env) {
                    return ExecutionContextResult::Abort;
                }
                stored_contracts.add_condition(recipe_item_idx, condition.clone());
            }
            _ => {
                unreachable!();
            }
        }
        ExecutionContextResult::Used
    }

    fn put_noninteractive_item_into_effect(
        recipe_id: usize,
        recipe_item_idx: ActionRecipeItemIdx,
        recipe_item: &ActionRecipeItem<C>,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
        stored_contracts: &mut ActionExecutionContractStore<C>,
    ) {
        debug_assert!(recipe_item.is_noninteractive());
        match recipe_item {
            ActionRecipeItem::EliminateItem(item_idx) => {
                stored_contracts.eliminate(
                    recipe_id,
                    item_idx,
                    command_list,
                    nest_recipe_command_list,
                );
            }
            ActionRecipeItem::StartEffect(effect) => {
                let cmd = effect.effect_start().clone();
                command_list.push(cmd);
                stored_contracts.add_effect(recipe_item_idx, effect.effect_end().clone());
            }
            ActionRecipeItem::StartEffectOf(effect_gen) => {
                let (effect_start, effect_end) = {
                    let exec_info = ActionRecipeExecutionInfo::new(stored_contracts);
                    (effect_gen)(exec_info)
                };
                command_list.push(effect_start);
                stored_contracts.add_effect(recipe_item_idx, effect_end);
            }
            ActionRecipeItem::StartNestRecipe(idx) => {
                nest_recipe_command_list.push(ActionNestRecipeCommand::Enable(recipe_id, *idx));
                stored_contracts.add_nest_recipe(recipe_item_idx, *idx);
            }
            ActionRecipeItem::DisableNestRecipe(idx) => {
                nest_recipe_command_list.push(ActionNestRecipeCommand::Disable(recipe_id, *idx));
                stored_contracts.add_nest_recipe_disabled(recipe_item_idx, *idx);
            }
            ActionRecipeItem::DoCommand(cmd) => {
                let cmd = cmd.command().clone();
                command_list.push(cmd);
            }
            ActionRecipeItem::DoCommandOf(cmd_gen) => {
                let exec_info = ActionRecipeExecutionInfo::new(stored_contracts);
                let cmd = (cmd_gen)(exec_info);
                command_list.push(cmd);
            }
            _ => unreachable!(),
        }
    }

    fn prepare_new_frame_for_compound_item(
        recipe_item: &ActionRecipeItem<C>,
        recipe_item_idx: ActionRecipeItemIdx,
    ) -> (ActionRecipeItemIdx, ActionExecutionFrame) {
        debug_assert!(recipe_item.is_compound());
        let frame = match recipe_item {
            ActionRecipeItem::Sequential(_) => ActionExecutionFrame::Sequential(None),
            ActionRecipeItem::Unordered(r) => ActionExecutionFrame::Unordered({
                let mut bitset = FixedBitSet::with_capacity(r.len());
                bitset.set_range(.., true);
                bitset
            }),
            ActionRecipeItem::Choice(_) => ActionExecutionFrame::Choice(None),
            _ => panic!("Primitive action item occured where only composite action item can occur"),
        };

        (recipe_item_idx, frame)
    }

    fn process_input_1(
        &mut self,
        input: &ActionInput<C>,
        recipe_items: &ActionRecipeItemStore<C>,
        recipe: &ActionRecipe<C>,
    ) -> ExecutionContextResult {
        if Self::stored_contracts_conflict(input, &mut self.stored_contracts) {
            return ExecutionContextResult::Abort;
        }

        let last_frame_depth = self.backtrace.len() - 1;
        let last_frame = self
            .backtrace
            .get_mut(last_frame_depth)
            .expect("Broken execution context data!");

        let seq = recipe_items.get(last_frame.0);
        debug_assert!(seq.is_compound());
        let seq_items = seq.compound_sequence();
        match &mut last_frame.1 {
            ActionExecutionFrame::Sequential(state_pos) => {
                let next = state_pos.map(|x| x + 1).unwrap_or(0);
                debug_assert!(next < seq_items.len());
                let seq_next_item_idx = seq_items[next];
                let seq_next_item = recipe_items.get(seq_next_item_idx);
                debug_assert!(seq_next_item.is_interactive());
                match Self::check_interactive_item_match_input(seq_next_item, input) {
                    ExecutionContextResult::Done => {
                        unreachable!();
                    }
                    ExecutionContextResult::Used => {
                        if self.recipe_idx == 0 {
                            debug!(target: "concerto", "process_input_1: recipe_id = {}, seq = {:?}, next = {}, used", self.recipe_idx, (last_frame.0), next);
                        }
                        self.stored_contracts
                            .add_input(seq_next_item_idx, input.clone());
                        *state_pos = Some(next);
                        return ExecutionContextResult::Used;
                    }
                    ExecutionContextResult::Ignore => {
                        return ExecutionContextResult::Ignore;
                    }
                    ExecutionContextResult::Abort => {
                        if next != 0 {
                            debug!(target: "concerto", "process_input_1: recipe_id = {}, seq = {:?}, next = {}, aborted", self.recipe_idx, (last_frame.0), next);
                        }
                        return ExecutionContextResult::Abort;
                    }
                }
            }
            ActionExecutionFrame::Unordered(state_set) => {
                debug_assert!(state_set.len() == seq_items.len());
                let mut update_item = None;
                'unordered_loop: for seq_idx in state_set.ones() {
                    let seq_next_item_idx = seq_items[seq_idx];
                    let seq_next_item = recipe_items.get(seq_next_item_idx);
                    debug_assert!(seq_next_item.is_interactive());
                    match Self::check_interactive_item_match_input(seq_next_item, input) {
                        ExecutionContextResult::Done => {
                            unreachable!();
                        }
                        ExecutionContextResult::Used => {
                            self.stored_contracts
                                .add_input(seq_next_item_idx, input.clone());
                            update_item = Some(seq_idx);
                            break 'unordered_loop;
                        }
                        ExecutionContextResult::Ignore => {}
                        ExecutionContextResult::Abort => {
                            return ExecutionContextResult::Abort;
                        }
                    }
                }
                if let Some(update_item) = update_item {
                    debug!(target: "concerto", "process_input_1: recipe_id = {}, seq = {:?}, unordered = {}, used", self.recipe_idx, (last_frame.0), update_item);
                    state_set.set(update_item, false);
                    return ExecutionContextResult::Used;
                } else {
                    return ExecutionContextResult::Ignore;
                }
            }
            ActionExecutionFrame::Choice(state_choice) => {
                debug_assert!(state_choice.is_none());
                let mut update_item = None;
                'choice_loop: for seq_idx in 0..(seq_items.len()) {
                    let seq_next_item_idx = seq_items[seq_idx];
                    let seq_next_item = recipe_items.get(seq_next_item_idx);
                    debug_assert!(seq_next_item.is_interactive());
                    match Self::check_interactive_item_match_input(seq_next_item, input) {
                        ExecutionContextResult::Done => {
                            unreachable!();
                        }
                        ExecutionContextResult::Used => {
                            self.stored_contracts
                                .add_input(seq_next_item_idx, input.clone());
                            update_item = Some(seq_idx);
                            break 'choice_loop;
                        }
                        ExecutionContextResult::Ignore => {}
                        ExecutionContextResult::Abort => {
                            return ExecutionContextResult::Abort;
                        }
                    }
                }
                if let Some(update_item) = update_item {
                    *state_choice = Some(update_item);
                    return ExecutionContextResult::Used;
                } else {
                    return ExecutionContextResult::Ignore;
                }
            }
            _ => unreachable!(),
        }
    }

    fn process_input_2(
        &mut self,
        recipe_items: &ActionRecipeItemStore<C>,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
        env: &ActionEnvironmentTrackingState<C>,
    ) -> ExecutionContextResult {
        'frame_loop: while !self.backtrace.is_empty() {
            let last_frame_depth = self.backtrace.len() - 1;
            let mut new_frame = None;
            {
                let last_frame = self
                    .backtrace
                    .get_mut(last_frame_depth)
                    .expect("Broken execution context data!");

                let seq = recipe_items.get(last_frame.0);
                debug_assert!(seq.is_compound());
                let seq_items = seq.compound_sequence();
                match &mut last_frame.1 {
                    ActionExecutionFrame::Sequential(state_pos) => {
                        let mut next = state_pos.map(|x| x + 1).unwrap_or(0);
                        'sequential_loop: while next < seq_items.len() {
                            let seq_next_item_idx = seq_items[next];
                            let seq_next_item = recipe_items.get(seq_next_item_idx);
                            if seq_next_item.is_interactive() {
                                debug!(target: "concerto", "process_input_2: recipe_id = {}, seq = {:?}, next = {}, stopped here", self.recipe_idx, last_frame.0, next);
                                return ExecutionContextResult::Used;
                            } else if seq_next_item.is_condition() {
                                match Self::check_condition_item_match_environment(
                                    seq_next_item_idx,
                                    seq_next_item,
                                    &mut self.stored_contracts,
                                    &env,
                                ) {
                                    ExecutionContextResult::Abort => {
                                        return ExecutionContextResult::Abort
                                    }
                                    _ => {}
                                }
                                *state_pos = Some(next);
                                next += 1;
                            } else if seq_next_item.is_noninteractive() {
                                debug!(target: "concerto", "process_input_2: recipe_id = {}, seq = {:?}, next = {}, non-interactive", self.recipe_idx, last_frame.0, next);
                                Self::put_noninteractive_item_into_effect(
                                    self.recipe_idx,
                                    seq_next_item_idx,
                                    seq_next_item,
                                    command_list,
                                    nest_recipe_command_list,
                                    &mut self.stored_contracts,
                                );
                                *state_pos = Some(next);
                                next += 1;
                            } else {
                                debug_assert!(seq_next_item.is_compound());
                                debug!(target: "concerto", "process_input_2: recipe_id = {}, seq = {:?}, next = {}, compound", self.recipe_idx, last_frame.0, next);
                                new_frame = Some(Self::prepare_new_frame_for_compound_item(
                                    seq_next_item,
                                    seq_next_item_idx,
                                ));
                                *state_pos = Some(next);
                                break 'sequential_loop;
                            }
                        }
                    }
                    ActionExecutionFrame::Unordered(state_set) => {
                        debug_assert!(state_set.len() == seq_items.len());

                        if let Some(first_unused) = state_set.ones().next() {
                            debug!(target: "concerto", "process_input_2: recipe_id = {}, seq = {:?}, unordered, first unmatch({}) stopped here", self.recipe_idx, last_frame.0, first_unused);
                            debug_assert!(
                                state_set
                                    .ones()
                                    .map(|seq_idx| recipe_items.get(seq_items[seq_idx]))
                                    .all(|x| x.is_interactive())
                            );
                            return ExecutionContextResult::Used;
                        } else {
                            debug!(target: "concerto", "process_input_2: recipe_id = {}, seq = {:?}, unordered, finished", self.recipe_idx, last_frame.0);
                        }
                    }
                    ActionExecutionFrame::Choice(state_choice) => {
                        if state_choice.is_none() {
                            debug_assert!(
                                (0..(seq_items.len()))
                                    .map(|seq_idx| recipe_items.get(seq_items[seq_idx]))
                                    .all(|x| x.is_interactive())
                            );
                            return ExecutionContextResult::Used;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            if let Some(new_frame) = new_frame {
                self.backtrace.push(new_frame);
            } else {
                self.backtrace.pop();
            }
        }
        ExecutionContextResult::Done
    }

    pub(crate) fn process_input(
        &mut self,
        input: &ActionInput<C>,
        recipe_items: &ActionRecipeItemStore<C>,
        recipe: &ActionRecipe<C>,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
        env: &ActionEnvironmentTrackingState<C>,
    ) -> ExecutionContextResult {
        match self.process_input_1(input, recipe_items, recipe) {
            ExecutionContextResult::Done => {
                unreachable!();
            }
            ExecutionContextResult::Used => {}
            ExecutionContextResult::Ignore => {
                return ExecutionContextResult::Ignore;
            }
            ExecutionContextResult::Abort => {
                return ExecutionContextResult::Abort;
            }
        }
        return self.process_input_2(recipe_items, command_list, nest_recipe_command_list, env);
    }

    pub(crate) fn clean_up(
        &mut self,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
    ) -> bool {
        self.stored_contracts
            .eliminate_all(self.recipe_idx, command_list, nest_recipe_command_list)
    }

    pub(crate) fn start_execution_with_input(
        input: &ActionInput<C>,
        recipe_items: &ActionRecipeItemStore<C>,
        recipe: &ActionRecipe<C>,
        recipe_idx: usize,
        command_list: &mut Vec<C::Command>,
        nest_recipe_command_list: &mut Vec<ActionNestRecipeCommand>,
        env: &ActionEnvironmentTrackingState<C>,
    ) -> (ExecutionContextResult, Option<Self>) {
        let mut exec_ctx = ActionExecutionCtx::new(recipe_idx, recipe, recipe_items);
        let mut temporary_nest_recipe_command_list = Vec::new();
        let result1 = exec_ctx.process_input_2(
            recipe_items,
            command_list,
            &mut temporary_nest_recipe_command_list,
            env,
        );
        match result1 {
            | ExecutionContextResult::Done => {
                panic!("You have a recipe that completes itself without any input!")
            }
            | ExecutionContextResult::Ignore | ExecutionContextResult::Abort => {
                return (ExecutionContextResult::Ignore, None);
            }
            | ExecutionContextResult::Used => {}
        }
        let result2 = exec_ctx.process_input(
            input,
            recipe_items,
            recipe,
            command_list,
            &mut temporary_nest_recipe_command_list,
            env,
        );
        match result2 {
            | ExecutionContextResult::Done => (ExecutionContextResult::Done, None),
            | ExecutionContextResult::Ignore | ExecutionContextResult::Abort => {
                (ExecutionContextResult::Ignore, None)
            }
            | ExecutionContextResult::Used => {
                nest_recipe_command_list.extend(temporary_nest_recipe_command_list.into_iter());
                (ExecutionContextResult::Used, Some(exec_ctx))
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum ExecutionContextResult {
    Done,
    Used,
    Ignore,
    Abort,
}

use context::ActionContextBuilder;
use context::ActionRecipeItemIdx;
use execution::ActionExecutionCtx;
use execution::ActionRecipeExecutionInfo;
use execution::ExecutionContextResult;
use smallvec::SmallVec;
use std::marker::PhantomData;

use ActionConfiguration;

pub struct ActionRecipe<C: ActionConfiguration> {
    pub(crate) root_item: ActionRecipeItemIdx,
    pub(crate) is_nested: bool,
    pub(crate) is_enabled: bool,
    pub(crate) nest_recipes: Vec<usize>,
    phantom: PhantomData<C>,
}

pub struct ActionRecipeCommand<C: ActionConfiguration>(C::Command);

impl<C: ActionConfiguration> ActionRecipeCommand<C> {
    pub(crate) fn command(&self) -> &C::Command {
        &self.0
    }
}

impl<C: ActionConfiguration> Clone for ActionRecipeCommand<C> {
    fn clone(&self) -> Self {
        ActionRecipeCommand(self.0.clone())
    }
}

pub struct ActionRecipeEffect<C: ActionConfiguration>(C::Command, C::Command);

impl<C: ActionConfiguration> ActionRecipeEffect<C> {
    pub(crate) fn effect_start(&self) -> &C::Command {
        &self.0
    }

    pub(crate) fn effect_end(&self) -> &C::Command {
        &self.1
    }
}

impl<C: ActionConfiguration> Clone for ActionRecipeEffect<C> {
    fn clone(&self) -> Self {
        ActionRecipeEffect(self.0.clone(), self.1.clone())
    }
}

pub(crate) enum ActionNestRecipeCommand {
    Enable(usize, usize),
    Disable(usize, usize),
    Abort(usize, usize),
}

pub(crate) enum ActionRecipeItem<C: ActionConfiguration> {
    StartInput(ActionInput<C>),
    StartFilteredInput(Rc<dyn Fn(&ActionInput<C>) -> ExecutionContextResult>),
    StartCondition(ActionCondition<C>),
    StartEffect(ActionRecipeEffect<C>),
    StartEffectOf(Box<dyn Fn(ActionRecipeExecutionInfo<C>) -> (C::Command, C::Command)>),
    StartNestRecipe(usize),
    DisableNestRecipe(usize),
    EliminateItem(ActionRecipeItemIdx),
    DoCommand(ActionRecipeCommand<C>),
    DoCommandOf(Box<dyn Fn(ActionRecipeExecutionInfo<C>) -> C::Command>),
    Sequential(SmallVec<[ActionRecipeItemIdx; 3]>),
    Unordered(SmallVec<[ActionRecipeItemIdx; 3]>),
    Choice(SmallVec<[ActionRecipeItemIdx; 3]>),
}

impl<C: ActionConfiguration> ActionRecipeItem<C> {
    pub(crate) fn is_interactive(&self) -> bool {
        match self {
            ActionRecipeItem::StartInput(_) => true,
            ActionRecipeItem::StartFilteredInput(_) => true,
            _ => false,
        }
    }

    pub(crate) fn is_condition(&self) -> bool {
        match self {
            ActionRecipeItem::StartCondition(_) => true,
            _ => false,
        }
    }

    pub(crate) fn is_noninteractive(&self) -> bool {
        match self {
            ActionRecipeItem::EliminateItem(_)
            | ActionRecipeItem::DoCommand(_)
            | ActionRecipeItem::DoCommandOf(_)
            | ActionRecipeItem::StartEffect(_)
            | ActionRecipeItem::StartEffectOf(_)
            | ActionRecipeItem::StartNestRecipe(_)
            | ActionRecipeItem::DisableNestRecipe(_) => true,
            _ => false,
        }
    }

    pub(crate) fn is_compound(&self) -> bool {
        match self {
            ActionRecipeItem::Sequential(_)
            | ActionRecipeItem::Unordered(_)
            | ActionRecipeItem::Choice(_) => true,
            _ => false,
        }
    }

    pub(crate) fn compound_sequence(&self) -> &[ActionRecipeItemIdx] {
        match self {
            ActionRecipeItem::Sequential(seq) => &seq,
            ActionRecipeItem::Unordered(seq) => &seq,
            ActionRecipeItem::Choice(seq) => &seq,
            _ => unreachable!(),
        }
    }
}

use std::rc::Rc;

pub enum ActionInput<C: ActionConfiguration> {
    CursorCoordinate(C::Target),
    FocusCoordinate(C::Target),
    KeyDown(C::KeyKind),
    KeyUp(C::KeyKind),
}

impl<C: ActionConfiguration> Clone for ActionInput<C> {
    fn clone(&self) -> Self {
        match self {
            ActionInput::CursorCoordinate(v) => ActionInput::CursorCoordinate(v.clone()),
            ActionInput::FocusCoordinate(v) => ActionInput::FocusCoordinate(v.clone()),
            ActionInput::KeyDown(v) => ActionInput::KeyDown(v.clone()),
            ActionInput::KeyUp(v) => ActionInput::KeyUp(v.clone()),
        }
    }
}

use std::fmt;

impl<C: ActionConfiguration> fmt::Debug for ActionInput<C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            ActionInput::CursorCoordinate(v) => write!(f, "CursorCoordinate({:?})", v),
            ActionInput::FocusCoordinate(v) => write!(f, "FocusCoordinate({:?})", v),
            ActionInput::KeyDown(v) => write!(f, "KeyDown({:?})", v),
            ActionInput::KeyUp(v) => write!(f, "KeyUp({:?})", v),
        }
    }
}

pub enum ActionCondition<C: ActionConfiguration> {
    KeyPressed(C::KeyKind, bool),
}

impl<C: ActionConfiguration> Clone for ActionCondition<C> {
    fn clone(&self) -> Self {
        match self {
            ActionCondition::KeyPressed(v, s) => ActionCondition::KeyPressed(v.clone(), s.clone()),
        }
    }
}

pub struct ActionRecipeBuilder<'a, C: ActionConfiguration> {
    sequence_builder: ActionRecipeSequenceBuilder<'a, C>,
    nest_recipes: Vec<ActionRecipe<C>>,
}

impl<'a, C: ActionConfiguration> ActionRecipeBuilder<'a, C> {
    pub(crate) fn new(context_builder: &'a mut ActionContextBuilder<C>) -> Self {
        ActionRecipeBuilder {
            sequence_builder: ActionRecipeSequenceBuilder::new(context_builder),
            nest_recipes: Vec::new(),
        }
    }
    pub fn build(self) -> ActionRecipe<C> {
        let (context_builder, sequence) = self.sequence_builder.build();
        let item_idx = context_builder.recipe_items.register_item(sequence);

        let mut nest_recipes = Vec::new();

        for nest_recipe in self.nest_recipes {
            let idx = context_builder.register_nested_recipe(nest_recipe);
            nest_recipes.push(idx);
        }

        ActionRecipe {
            root_item: item_idx,
            phantom: PhantomData,
            is_enabled: true,
            is_nested: false,
            nest_recipes,
        }
    }
}

impl<'a, C: ActionConfiguration> ActionRecipeBuilder<'a, C> {
    pub fn keep_cursor_coordinate_input(mut self, target: C::Target) -> Self {
        self.sequence_builder
            .add_primitive_start_cursor_coordinate_input(target);
        self
    }

    pub fn keep_cursor_coordinate_filtered_input<F>(mut self, filter: F) -> Self
    where
        F: Fn(&C::Target) -> bool + 'static,
    {
        self.sequence_builder
            .add_primitive_start_cursor_coordinate_filtered_input(filter);
        self
    }

    pub fn add_cursor_coordinate_filtered_input<F>(mut self, filter: F) -> Self
    where
        F: Fn(&C::Target) -> bool + 'static,
    {
        let input_idx = self
            .sequence_builder
            .add_primitive_start_cursor_coordinate_filtered_input(filter);
        self.sequence_builder
            .add_primitive_eliminate_item(input_idx);
        self
    }

    pub fn keep_key_not_pressed(mut self, key: C::KeyKind) -> Self {
        self.sequence_builder
            .add_primitive_start_key_condition(key, false);
        self
    }

    pub fn check_key_pressed(mut self, key: C::KeyKind) -> Self {
        let input_idx = self
            .sequence_builder
            .add_primitive_start_key_condition(key, true);
        self.sequence_builder
            .add_primitive_eliminate_item(input_idx);
        self
    }
    pub fn add_key_down_input(mut self, key: C::KeyKind) -> Self {
        let input_idx = self
            .sequence_builder
            .add_primitive_start_key_down_input(key);
        self.sequence_builder
            .add_primitive_eliminate_item(input_idx);
        self
    }
    pub fn add_key_up_input(mut self, key: C::KeyKind) -> Self {
        let input_idx = self.sequence_builder.add_primitive_start_key_up_input(key);
        self.sequence_builder
            .add_primitive_eliminate_item(input_idx);
        self
    }

    pub fn enable_starting_nest_recipe<F>(mut self, f: F) -> Self
    where
        F: for<'r> FnOnce(usize, ActionRecipeBuilder<'r, C>) -> ActionRecipe<C>,
    {
        let nest_recipe_idx = self.nest_recipes.len();
        let nest_recipe = {
            let nest_recipe_builder =
                ActionRecipeBuilder::new(self.sequence_builder.context_builder);
            (f)(nest_recipe_idx, nest_recipe_builder)
        };
        self.nest_recipes.push(nest_recipe);
        self.sequence_builder
            .add_primitive_start_nest_recipe(nest_recipe_idx);
        self
    }

    pub fn disable_starting_nest_recipe(mut self, nest_recipe_idx: usize) -> Self {
        self.sequence_builder
            .add_primitive_disable_nest_recipe(nest_recipe_idx);
        self
    }

    pub fn issue_command(mut self, command: C::Command) -> Self {
        self.sequence_builder.add_primitive_issue_command(command);
        self
    }

    pub fn issue_command_with<F>(mut self, command_generator: F) -> Self
    where
        F: Fn(ActionRecipeExecutionInfo<C>) -> C::Command + 'static,
    {
        self.sequence_builder
            .add_primitive_issue_command_with(command_generator);
        self
    }

    pub fn issue_effect(mut self, effect_start: C::Command, effect_end: C::Command) -> Self {
        self.sequence_builder
            .add_primitive_issue_effect(effect_start, effect_end);
        self
    }

    pub fn issue_effect_with<F>(mut self, effect_generator: F) -> Self
    where
        F: Fn(ActionRecipeExecutionInfo<C>) -> (C::Command, C::Command) + 'static,
    {
        self.sequence_builder
            .add_primitive_issue_effect_with(effect_generator);
        self
    }

    pub fn add_sequential_multiple_key_down_input(mut self, keys: &[C::KeyKind]) -> Self {
        let mut items = None;
        self.sequence_builder.add_compound_sequence(
            ActionRecipeSequenceKind::Sequential,
            |builder| {
                items = Some(
                    keys.iter()
                        .map(|key| builder.add_primitive_start_key_down_input(key.clone()))
                        .collect::<Vec<_>>(),
                );
            },
        );

        if let Some(items) = items {
            self.sequence_builder.add_compound_sequence(
                ActionRecipeSequenceKind::Sequential,
                |builder| {
                    items.into_iter().for_each(|item| {
                        builder.add_primitive_eliminate_item(item);
                    });
                },
            );
        }
        self
    }
    pub fn add_unordered_multiple_key_down_input(mut self, keys: &[C::KeyKind]) -> Self {
        let mut items = None;
        self.sequence_builder.add_compound_sequence(
            ActionRecipeSequenceKind::Unordered,
            |builder| {
                items = Some(
                    keys.iter()
                        .map(|key| builder.add_primitive_start_key_down_input(key.clone()))
                        .collect::<Vec<_>>(),
                );
            },
        );

        if let Some(items) = items {
            self.sequence_builder.add_compound_sequence(
                ActionRecipeSequenceKind::Sequential,
                |builder| {
                    items.into_iter().for_each(|item| {
                        builder.add_primitive_eliminate_item(item);
                    });
                },
            );
        }
        self
    }
    pub fn add_unordered_multiple_key_up_input(mut self, keys: &[C::KeyKind]) -> Self {
        let mut items = None;
        self.sequence_builder.add_compound_sequence(
            ActionRecipeSequenceKind::Unordered,
            |builder| {
                items = Some(
                    keys.iter()
                        .map(|key| builder.add_primitive_start_key_up_input(key.clone()))
                        .collect::<Vec<_>>(),
                );
            },
        );

        if let Some(items) = items {
            self.sequence_builder.add_compound_sequence(
                ActionRecipeSequenceKind::Sequential,
                |builder| {
                    items.into_iter().for_each(|item| {
                        builder.add_primitive_eliminate_item(item);
                    });
                },
            );
        }
        self
    }

    pub fn add_one_of_multiple_key_up_input(mut self, keys: &[C::KeyKind]) -> Self {
        let mut items = None;
        self.sequence_builder
            .add_compound_sequence(ActionRecipeSequenceKind::Choice, |builder| {
                items = Some(
                    keys.iter()
                        .map(|key| builder.add_primitive_start_key_up_input(key.clone()))
                        .collect::<Vec<_>>(),
                );
            });

        if let Some(items) = items {
            self.sequence_builder.add_compound_sequence(
                ActionRecipeSequenceKind::Sequential,
                |builder| {
                    items.into_iter().for_each(|item| {
                        builder.add_primitive_eliminate_item(item);
                    });
                },
            );
        }
        self
    }
}

enum ActionRecipeSequenceKind {
    Sequential,
    Unordered,
    Choice,
}

struct ActionRecipeSequenceBuilder<'a, C: ActionConfiguration> {
    kind: ActionRecipeSequenceKind,
    context_builder: &'a mut ActionContextBuilder<C>,
    item_idxes: SmallVec<[ActionRecipeItemIdx; 3]>,
}

impl<'a, C: ActionConfiguration> ActionRecipeSequenceBuilder<'a, C> {
    fn new(context_builder: &'a mut ActionContextBuilder<C>) -> Self {
        ActionRecipeSequenceBuilder {
            kind: ActionRecipeSequenceKind::Sequential,
            context_builder,
            item_idxes: SmallVec::new(),
        }
    }

    fn new_inner<'b>(
        parent_builder: &'a mut ActionRecipeSequenceBuilder<'b, C>,
        kind: ActionRecipeSequenceKind,
    ) -> Self {
        ActionRecipeSequenceBuilder {
            kind,
            context_builder: parent_builder.context_builder,
            item_idxes: SmallVec::new(),
        }
    }

    fn add_recipe_item(&mut self, item_idx: ActionRecipeItemIdx) {
        self.item_idxes.push(item_idx);
    }

    fn add_primitive_start_cursor_coordinate_input(
        &mut self,
        target: C::Target,
    ) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::StartInput(ActionInput::CursorCoordinate(target));
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_start_cursor_coordinate_filtered_input<F>(
        &mut self,
        filter: F,
    ) -> ActionRecipeItemIdx
    where
        F: Fn(&C::Target) -> bool + 'static,
    {
        let input = ActionRecipeItem::StartFilteredInput(Rc::new(
            ActionExecutionCtx::make_input_filter_with_cursor_coordinate_filter(filter),
        ) as _);
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_start_key_down_input(&mut self, key: C::KeyKind) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::StartInput(ActionInput::KeyDown(key));
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_start_key_up_input(&mut self, key: C::KeyKind) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::StartInput(ActionInput::KeyUp(key));
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_start_key_condition(
        &mut self,
        key: C::KeyKind,
        pressed: bool,
    ) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::StartCondition(ActionCondition::KeyPressed(key, pressed));
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_start_nest_recipe(&mut self, nest_recipe: usize) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::StartNestRecipe(nest_recipe);
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_disable_nest_recipe(&mut self, nest_recipe: usize) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::DisableNestRecipe(nest_recipe);
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn add_primitive_eliminate_item(&mut self, item: ActionRecipeItemIdx) -> ActionRecipeItemIdx {
        let input = ActionRecipeItem::EliminateItem(item);
        let item_idx = self.context_builder.recipe_items.register_item(input);
        self.add_recipe_item(item_idx);
        item_idx
    }

    pub fn add_primitive_issue_command(&mut self, command: C::Command) -> ActionRecipeItemIdx {
        let command = ActionRecipeItem::DoCommand(ActionRecipeCommand(command));
        let item_idx = self.context_builder.recipe_items.register_item(command);
        self.add_recipe_item(item_idx);
        item_idx
    }

    pub fn add_primitive_issue_command_with<F>(
        &mut self,
        command_generator: F,
    ) -> ActionRecipeItemIdx
    where
        F: Fn(ActionRecipeExecutionInfo<C>) -> C::Command + 'static,
    {
        let command_of = ActionRecipeItem::DoCommandOf(Box::new(command_generator) as _);
        let item_idx = self.context_builder.recipe_items.register_item(command_of);
        self.add_recipe_item(item_idx);
        item_idx
    }

    pub fn add_primitive_issue_effect(
        &mut self,
        effect_start: C::Command,
        effect_end: C::Command,
    ) -> ActionRecipeItemIdx {
        let command = ActionRecipeItem::StartEffect(ActionRecipeEffect(effect_start, effect_end));
        let item_idx = self.context_builder.recipe_items.register_item(command);
        self.add_recipe_item(item_idx);
        item_idx
    }

    pub fn add_primitive_issue_effect_with<F>(&mut self, effect_generator: F) -> ActionRecipeItemIdx
    where
        F: Fn(ActionRecipeExecutionInfo<C>) -> (C::Command, C::Command) + 'static,
    {
        let effect_of = ActionRecipeItem::StartEffectOf(Box::new(effect_generator) as _);
        let item_idx = self.context_builder.recipe_items.register_item(effect_of);
        self.add_recipe_item(item_idx);
        item_idx
    }

    pub fn add_compound_sequence<F>(
        &mut self,
        kind: ActionRecipeSequenceKind,
        sequence_generator: F,
    ) -> ActionRecipeItemIdx
    where
        F: for<'r> FnOnce(&mut ActionRecipeSequenceBuilder<'r, C>),
    {
        let sequence = {
            let mut builder = ActionRecipeSequenceBuilder::new_inner(self, kind);
            (sequence_generator)(&mut builder);
            builder.build().1
        };
        let item_idx = self.context_builder.recipe_items.register_item(sequence);
        self.add_recipe_item(item_idx);
        item_idx
    }

    fn build(self) -> (&'a mut ActionContextBuilder<C>, ActionRecipeItem<C>) {
        (
            self.context_builder,
            match self.kind {
                ActionRecipeSequenceKind::Sequential => {
                    ActionRecipeItem::Sequential(self.item_idxes)
                }
                ActionRecipeSequenceKind::Unordered => ActionRecipeItem::Unordered(self.item_idxes),
                ActionRecipeSequenceKind::Choice => ActionRecipeItem::Choice(self.item_idxes),
            },
        )
    }
}

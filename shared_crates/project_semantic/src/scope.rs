use std::path::PathBuf;

use ambient_project::{ComponentType, Identifier, ItemPath, Manifest};
use anyhow::Context as AnyhowContext;
use indexmap::IndexMap;

use crate::{
    Attribute, Component, Concept, Item, ItemData, ItemId, ItemMap, ItemType, ItemValue, Message,
    Resolve, ResolveClone, Type,
};

#[derive(Clone, PartialEq, Debug)]
pub struct Context(Vec<ItemId<Scope>>);
impl Context {
    pub(crate) fn new(root_scope: ItemId<Scope>) -> Self {
        Self(vec![root_scope])
    }

    fn push(&mut self, scope: ItemId<Scope>) {
        self.0.push(scope);
    }

    pub(crate) fn get_type_id(
        &self,
        items: &ItemMap,
        component_type: &ComponentType,
    ) -> Option<ItemId<Type>> {
        for &scope_id in self.0.iter().rev() {
            match component_type {
                ComponentType::Item(id) => {
                    if let Ok(id) = get_type_id(items, scope_id, id.as_path()) {
                        return Some(id);
                    }
                }
                ComponentType::Contained {
                    type_,
                    element_type,
                } => {
                    if let Ok(id) = get_type_id(items, scope_id, element_type.as_path()) {
                        return Some(match type_ {
                            ambient_project::ContainerType::Vec => items.get_vec_id(id),
                            ambient_project::ContainerType::Option => items.get_option_id(id),
                        });
                    }
                }
            }
        }
        None
    }

    pub(crate) fn get_attribute_id(
        &self,
        items: &ItemMap,
        path: ItemPath,
    ) -> anyhow::Result<ItemId<Attribute>> {
        for &scope_id in self.0.iter().rev() {
            if let Ok(id) = get_attribute_id(items, scope_id, path) {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to find attribute {:?}", path);
    }

    pub(crate) fn get_concept_id(
        &self,
        items: &ItemMap,
        path: ItemPath,
    ) -> anyhow::Result<ItemId<Concept>> {
        for &scope_id in self.0.iter().rev() {
            if let Ok(id) = get_concept_id(items, scope_id, path) {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to find concept {:?}", path);
    }

    pub(crate) fn get_component_id(
        &self,
        items: &ItemMap,
        path: ItemPath,
    ) -> anyhow::Result<ItemId<Component>> {
        for &scope_id in self.0.iter().rev() {
            if let Ok(id) = get_component_id(items, scope_id, path) {
                return Ok(id);
            }
        }
        anyhow::bail!("failed to find component {:?}", path);
    }
}

#[derive(Clone, PartialEq)]
pub struct Scope {
    pub data: ItemData,
    pub path: Option<PathBuf>,
    pub manifest: Option<Manifest>,

    pub scopes: IndexMap<Identifier, ItemId<Scope>>,
    pub components: IndexMap<Identifier, ItemId<Component>>,
    pub concepts: IndexMap<Identifier, ItemId<Concept>>,
    pub messages: IndexMap<Identifier, ItemId<Message>>,
    pub types: IndexMap<Identifier, ItemId<Type>>,
    pub attributes: IndexMap<Identifier, ItemId<Attribute>>,
}
impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ds = f.debug_struct("Scope");
        ds.field("data", &self.data);

        if let Some(path) = &self.path {
            ds.field("path", &path);
        }
        if !self.components.is_empty() {
            ds.field("components", &self.components);
        }
        if !self.concepts.is_empty() {
            ds.field("concepts", &self.concepts);
        }
        if !self.messages.is_empty() {
            ds.field("messages", &self.messages);
        }
        if !self.types.is_empty() {
            ds.field("types", &self.types);
        }
        if !self.attributes.is_empty() {
            ds.field("attributes", &self.attributes);
        }
        if !self.scopes.is_empty() {
            ds.field("scopes", &self.scopes);
        }

        ds.finish()
    }
}
impl Item for Scope {
    const TYPE: ItemType = ItemType::Scope;

    type Unresolved = ();

    fn from_item_value(value: &ItemValue) -> Option<&Self> {
        match value {
            ItemValue::Scope(value) => Some(value),
            _ => None,
        }
    }

    fn from_item_value_mut(value: &mut ItemValue) -> Option<&mut Self> {
        match value {
            ItemValue::Scope(value) => Some(value),
            _ => None,
        }
    }

    fn into_item_value(self) -> ItemValue {
        ItemValue::Scope(self)
    }

    fn data(&self) -> &ItemData {
        &self.data
    }
}
/// Scope uses `ResolveClone` because scopes can be accessed during resolution
/// of their children, so we need to clone the scope to avoid a double-borrow.
impl ResolveClone for Scope {
    fn resolve_clone(
        self,
        items: &mut ItemMap,
        self_id: ItemId<Self>,
        context: &Context,
    ) -> anyhow::Result<Self> {
        fn resolve<T: Resolve, U>(
            item_ids: &IndexMap<U, ItemId<T>>,
            items: &ItemMap,
            context: &Context,
        ) -> anyhow::Result<()> {
            for id in item_ids.values().copied() {
                items.resolve(id, context)?;
            }

            Ok(())
        }

        let mut context = context.clone();
        context.push(self_id);

        for id in self.scopes.values() {
            items.resolve_clone(*id, &context)?;
        }
        resolve(&self.components, items, &context)?;
        resolve(&self.concepts, items, &context)?;
        resolve(&self.messages, items, &context)?;
        resolve(&self.types, items, &context)?;
        resolve(&self.attributes, items, &context)?;

        Ok(self)
    }
}
impl Scope {
    /// Creates a new empty scope with the specified data.
    pub fn new(data: ItemData, path: Option<PathBuf>, manifest: Option<Manifest>) -> Self {
        Self {
            data,
            path,
            manifest,
            scopes: Default::default(),
            components: Default::default(),
            concepts: Default::default(),
            messages: Default::default(),
            types: Default::default(),
            attributes: Default::default(),
        }
    }

    pub fn visit_recursive(
        &self,
        items: &ItemMap,
        mut visitor: impl FnMut(&Scope) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        fn visit_recursive_inner(
            scope: &Scope,
            items: &ItemMap,
            visitor: &mut dyn FnMut(&Scope) -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            visitor(scope)?;

            for scope in scope.scopes.values().copied() {
                visit_recursive_inner(&*items.get(scope)?, items, visitor)?;
            }

            Ok(())
        }

        visit_recursive_inner(self, items, &mut visitor)
    }
}

fn get_type_id(
    items: &ItemMap,
    self_scope_id: ItemId<Scope>,
    path: ItemPath,
) -> anyhow::Result<ItemId<Type>> {
    let (scope, item) = path.scope_and_item();
    items
        .get_scope(self_scope_id, scope.iter())?
        .types
        .get(item)
        .copied()
        .with_context(|| format!("failed to find type {item} in {scope:?}"))
}

fn get_attribute_id(
    items: &ItemMap,
    self_scope_id: ItemId<Scope>,
    path: ItemPath,
) -> anyhow::Result<ItemId<Attribute>> {
    let (scope, item) = path.scope_and_item();
    items
        .get_scope(self_scope_id, scope.iter())?
        .attributes
        .get(item)
        .copied()
        .with_context(|| format!("failed to find attribute {item} in {scope:?}",))
}

fn get_concept_id(
    items: &ItemMap,
    self_scope_id: ItemId<Scope>,
    path: ItemPath,
) -> anyhow::Result<ItemId<Concept>> {
    let (scope, item) = path.scope_and_item();
    items
        .get_scope(self_scope_id, scope.iter())?
        .concepts
        .get(item)
        .copied()
        .with_context(|| format!("failed to find concept {item} in {scope:?}",))
}

fn get_component_id(
    items: &ItemMap,
    self_scope_id: ItemId<Scope>,
    path: ItemPath,
) -> anyhow::Result<ItemId<Component>> {
    let (scope, item) = path.scope_and_item();
    items
        .get_scope(self_scope_id, scope.iter())?
        .components
        .get(item)
        .copied()
        .with_context(|| format!("failed to find component {item} in {scope:?}",))
}

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

use ambient_project::{Dependency, Identifier, Manifest};
use ambient_shared_types::primitive_component_definitions;
use anyhow::Context as AnyhowContext;
use convert_case::{Boundary, Case, Casing};

mod scope;
pub use scope::{Context, Scope};

mod item;
pub use item::{Item, ItemData, ItemId, ItemMap, ItemType, ItemValue, ResolvableItemId};
use item::{Resolve, ResolveClone};

mod component;
pub use component::Component;

mod concept;
pub use concept::Concept;

mod attribute;
pub use attribute::Attribute;

mod primitive_type;
pub use primitive_type::PrimitiveType;

mod type_;
pub use type_::{Enum, Type, TypeInner};

mod message;
pub use message::Message;

mod value;
pub use value::{PrimitiveValue, ResolvableValue, ResolvedValue};

pub trait FileProvider {
    fn get(&self, path: &Path) -> std::io::Result<String>;
    fn full_path(&self, path: &Path) -> PathBuf;
}

pub struct ProxyFileProvider<'a> {
    pub provider: &'a dyn FileProvider,
    pub base: &'a Path,
}
impl FileProvider for ProxyFileProvider<'_> {
    fn get(&self, path: &Path) -> std::io::Result<String> {
        self.provider.get(&self.base.join(path))
    }

    fn full_path(&self, path: &Path) -> PathBuf {
        ambient_shared_types::path::normalize(&self.provider.full_path(&self.base.join(path)))
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct Semantic {
    pub items: ItemMap,
    pub root_scope: ItemId<Scope>,
}
impl Semantic {
    pub fn new() -> anyhow::Result<Self> {
        let mut items = ItemMap::default();
        let root_scope = create_root_scope(&mut items)?;
        Ok(Self { items, root_scope })
    }

    pub fn add_file_at_non_toplevel(
        &mut self,
        parent_scope: ItemId<Scope>,
        filename: &Path,
        file_provider: &dyn FileProvider,
        is_ambient: bool,
    ) -> anyhow::Result<ItemId<Scope>> {
        let manifest = Manifest::parse(&file_provider.get(filename)?)
            .with_context(|| format!("failed to parse toml for {filename:?}"))?;

        let id = manifest.ember.id.clone();
        self.add_scope_from_manifest(
            Some(parent_scope),
            file_provider,
            manifest,
            file_provider.full_path(filename),
            id,
            is_ambient,
        )
    }

    // TODO(philpax): This merges organizations together, which may lead to some degree of semantic conflation,
    // especially with dependencies: a parent may be able to access a child's dependencies.
    //
    // This is a simplifying assumption that will enable the cross-cutting required for Ambient's ecosystem,
    // but will lead to unexpected behaviour in future.
    //
    // A fix may be to treat each added manifest as an "island", and then have the resolution step
    // jump between islands as required to resolve things. There are a couple of nuances here that
    // I decided to push to another day in the interest of getting this working.
    //
    // These nuances include:
    // - Sharing the same "ambient" types between islands (primitive types, Ambient API)
    // - If one module/island (P) has dependencies on two islands (A, B), both of which have a shared dependency (C),
    //   both A and B should have the same C and not recreate it. C should not be visible from P.
    // - Local changes should not have global effects, unless they are globally visible. If, using the above configuration,
    //   a change occurs to C, there should be absolutely no impact on P if P does not depend on C.
    //
    // At the present, there's just one big island, so P can see C, and changes to C will affect P.
    pub fn add_file(
        &mut self,
        filename: &Path,
        file_provider: &dyn FileProvider,
        is_ambient: bool,
    ) -> anyhow::Result<ItemId<Scope>> {
        let manifest = Manifest::parse(&file_provider.get(filename)?)
            .with_context(|| format!("failed to parse toml for {filename:?}"))?;

        // Create an organization scope if necessary
        let organization_key = manifest.ember.organization.as_ref().with_context(|| {
            format!(
                "file {:?} has no organization, which is required for a top-level ember",
                file_provider.full_path(filename)
            )
        })?;

        let organization_id = self
            .items
            .get(self.root_scope)?
            .scopes
            .get(organization_key)
            .map(|(_, id)| *id);
        let organization_id = organization_id.unwrap_or_else(|| {
            let id = self.items.add(Scope::new(ItemData {
                parent_id: Some(self.root_scope),
                id: organization_key.clone(),
                is_ambient: false,
            }));

            self.items
                .get_mut(self.root_scope)
                .unwrap()
                .scopes
                .insert(organization_key.clone(), (Default::default(), id));

            id
        });

        // Check that this scope hasn't already been created for this organization
        let scope_id = manifest.ember.id.clone();
        if let Some((existing_path, existing_scope_id)) =
            self.items.get(organization_id)?.scopes.get(&scope_id)
        {
            if existing_path == &file_provider.full_path(filename) {
                return Ok(*existing_scope_id);
            }

            anyhow::bail!(
                "attempted to add {:?}, but a scope already exists at `{scope_id}`",
                file_provider.full_path(filename)
            );
        }

        // Create a new scope and add it to the organization
        let manifest_path = file_provider.full_path(filename);
        let item_id = self.add_scope_from_manifest(
            Some(organization_id),
            file_provider,
            manifest,
            manifest_path.clone(),
            scope_id.clone(),
            is_ambient,
        )?;
        self.items
            .get_mut(organization_id)?
            .scopes
            .insert(scope_id, (manifest_path, item_id));
        Ok(item_id)
    }

    pub fn resolve(&mut self) -> anyhow::Result<()> {
        let root_scopes = self
            .items
            .get(self.root_scope)?
            .scopes
            .values()
            .map(|(_, id)| *id)
            .collect::<Vec<_>>();

        for scope_id in root_scopes {
            self.items
                .resolve_clone(scope_id, &Context::new(self.root_scope))?;
        }
        Ok(())
    }
}
impl Semantic {
    fn add_scope_from_manifest(
        &mut self,
        parent_id: Option<ItemId<Scope>>,
        file_provider: &dyn FileProvider,
        manifest: Manifest,
        manifest_path: PathBuf,
        id: Identifier,
        is_ambient: bool,
    ) -> anyhow::Result<ItemId<Scope>> {
        let scope = Scope::new(ItemData {
            parent_id,
            id,
            is_ambient,
        });
        let scope_id = self.items.add(scope);

        for include in &manifest.ember.includes {
            let child_scope_id =
                self.add_file_at_non_toplevel(scope_id, include, file_provider, is_ambient)?;
            let id = self.items.get(child_scope_id)?.data().id.clone();
            self.items
                .get_mut(scope_id)?
                .scopes
                .insert(id, (file_provider.full_path(include), child_scope_id));
        }

        for (_, dependency) in manifest.dependencies.iter() {
            match dependency {
                Dependency::Path { path } => {
                    let file_provider = ProxyFileProvider {
                        provider: file_provider,
                        base: &path,
                    };

                    self.add_file(Path::new("ambient.toml"), &file_provider, is_ambient)?;
                }
            }
        }

        let make_item_data = |item_id: &Identifier| -> ItemData {
            ItemData {
                parent_id: Some(scope_id),
                id: item_id.clone(),
                is_ambient,
            }
        };

        let items = &mut self.items;
        for (path, component) in manifest.components.iter() {
            let path = path.as_path();
            let (scope_path, item) = path.scope_and_item();

            let value = items.add(Component::from_project(make_item_data(item), component));
            items
                .get_or_create_scope_mut(manifest_path.clone(), scope_id, scope_path)?
                .components
                .insert(item.clone(), value);
        }

        for (path, concept) in manifest.concepts.iter() {
            let path = path.as_path();
            let (scope_path, item) = path.scope_and_item();

            let value = items.add(Concept::from_project(make_item_data(item), concept));
            items
                .get_or_create_scope_mut(manifest_path.clone(), scope_id, scope_path)?
                .concepts
                .insert(item.clone(), value);
        }

        for (path, message) in manifest.messages.iter() {
            let path = path.as_path();
            let (scope_path, item) = path.scope_and_item();

            let value = items.add(Message::from_project(make_item_data(item), message));
            items
                .get_or_create_scope_mut(manifest_path.clone(), scope_id, scope_path)?
                .messages
                .insert(item.clone(), value);
        }

        for (segment, enum_ty) in manifest.enums.iter() {
            let enum_id = items.add(Type::from_project_enum(make_item_data(segment), enum_ty));
            items
                .get_mut(scope_id)?
                .types
                .insert(segment.clone(), enum_id);
        }

        Ok(scope_id)
    }
}

fn create_root_scope(items: &mut ItemMap) -> anyhow::Result<ItemId<Scope>> {
    macro_rules! define_primitive_types {
        ($(($value:ident, $_type:ty)),*) => {
            [
                $((stringify!($value), PrimitiveType::$value)),*
            ]
        };
    }

    let root_scope = items.add(Scope::new(ItemData {
        parent_id: None,
        id: Identifier::default(),
        is_ambient: true,
    }));

    for (id, pt) in primitive_component_definitions!(define_primitive_types) {
        let id = id
            .with_boundaries(&[
                Boundary::LowerUpper,
                Boundary::DigitUpper,
                Boundary::DigitLower,
                Boundary::Acronym,
            ])
            .to_case(Case::Kebab);
        let id = Identifier::new(id)
            .map_err(anyhow::Error::msg)
            .context("standard value was not valid kebab-case")?;

        let ty = Type::new(
            ItemData {
                parent_id: Some(root_scope),
                id: id.clone(),
                is_ambient: true,
            },
            TypeInner::Primitive(pt),
        );
        let item_id = items.add(ty);
        items.get_mut(root_scope)?.types.insert(id, item_id);
    }

    for name in [
        "debuggable",
        "networked",
        "resource",
        "maybe-resource",
        "store",
    ] {
        let id = Identifier::new(name)
            .map_err(anyhow::Error::msg)
            .context("standard value was not valid kebab-case")?;
        let item_id = items.add(Attribute {
            data: ItemData {
                parent_id: Some(root_scope),
                id: id.clone(),
                is_ambient: true,
            },
        });
        items.get_mut(root_scope)?.attributes.insert(id, item_id);
    }

    Ok(root_scope)
}

pub struct Printer {
    indent: usize,
}
impl Printer {
    pub fn new() -> Self {
        Self { indent: 0 }
    }

    pub fn print(&mut self, semantic: &Semantic) -> anyhow::Result<()> {
        let items = &semantic.items;
        self.print_scope(items, &*items.get(semantic.root_scope)?)?;
        Ok(())
    }

    fn print_scope(&mut self, items: &ItemMap, scope: &Scope) -> anyhow::Result<()> {
        for id in scope.components.values() {
            self.print_component(items, &*items.get(*id)?)?;
        }

        for id in scope.concepts.values() {
            self.print_concept(items, &*items.get(*id)?)?;
        }

        for id in scope.messages.values() {
            self.print_message(items, &*items.get(*id)?)?;
        }

        for id in scope.types.values() {
            self.print_type(items, &*items.get(*id)?)?;
        }

        for id in scope.attributes.values() {
            self.print_attribute(items, &*items.get(*id)?)?;
        }

        for (_, id) in scope.scopes.values() {
            self.print_scope(items, &*items.get(*id)?)?;
        }

        Ok(())
    }

    fn print_component(&mut self, items: &ItemMap, component: &Component) -> anyhow::Result<()> {
        self.print_indent();
        println!("{}", fully_qualified_path(items, component)?);

        self.with_indent(|p| {
            p.print_indent();
            println!("name: {:?}", component.name.as_deref().unwrap_or_default());

            p.print_indent();
            println!(
                "description: {:?}",
                component.description.as_deref().unwrap_or_default()
            );

            p.print_indent();
            println!("type: {}", write_resolvable_id(items, &component.type_)?);

            p.print_indent();
            println!("attributes:");
            p.with_indent(|p| {
                for attribute in &component.attributes {
                    p.print_indent();
                    println!("{}", write_resolvable_id(items, attribute)?);
                }
                Ok(())
            })?;

            p.print_indent();
            println!("default: {:?}", component.default);

            Ok(())
        })
    }

    fn print_concept(&mut self, items: &ItemMap, concept: &Concept) -> anyhow::Result<()> {
        self.print_indent();
        println!("{}", fully_qualified_path(items, concept)?);

        self.with_indent(|p| {
            p.print_indent();
            println!("name: {:?}", concept.name.as_deref().unwrap_or_default());

            p.print_indent();
            println!(
                "description: {:?}",
                concept.description.as_deref().unwrap_or_default()
            );

            p.print_indent();
            print!("extends:");
            for extend in &concept.extends {
                print!("{} ", write_resolvable_id(items, extend)?);
            }
            println!();

            p.print_indent();
            println!("components:");

            p.with_indent(|p| {
                for (component, value) in concept.components.iter() {
                    p.print_indent();
                    println!("{}: {:?}", write_resolvable_id(items, component)?, value,);
                }

                Ok(())
            })
        })
    }

    fn print_message(&mut self, items: &ItemMap, message: &Message) -> anyhow::Result<()> {
        self.print_indent();
        println!("{}", fully_qualified_path(items, message)?);

        self.with_indent(|p| {
            p.print_indent();
            println!(
                "description: {:?}",
                message.description.as_deref().unwrap_or_default()
            );

            p.print_indent();
            println!("fields:");

            p.with_indent(|p| {
                for (id, ty) in message.fields.iter() {
                    p.print_indent();
                    println!("{}: {}", id, write_resolvable_id(items, ty)?);
                }

                Ok(())
            })
        })
    }

    fn print_type(&mut self, items: &ItemMap, type_: &Type) -> anyhow::Result<()> {
        self.print_indent();
        println!("{}", fully_qualified_path(items, type_)?,);
        if let TypeInner::Enum(e) = &type_.inner {
            self.with_indent(|p| {
                for (name, description) in &e.members {
                    p.print_indent();
                    print!("{name}: {description}");
                    println!();
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn print_attribute(&mut self, items: &ItemMap, attribute: &Attribute) -> anyhow::Result<()> {
        self.print_indent();
        println!("{}", fully_qualified_path(items, attribute)?);
        Ok(())
    }

    fn print_indent(&self) {
        for _ in 0..self.indent {
            print!("  ");
        }
    }

    fn with_indent(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.indent += 1;
        f(self)?;
        self.indent -= 1;
        Ok(())
    }
}

fn write_resolvable_id<T: Item>(
    items: &ItemMap,
    r: &ResolvableItemId<T>,
) -> anyhow::Result<String> {
    Ok(match r {
        ResolvableItemId::Unresolved(unresolved) => format!("unresolved({:?})", unresolved),
        ResolvableItemId::Resolved(resolved) => {
            format!("{}", fully_qualified_path(items, &*items.get(*resolved)?)?)
        }
    })
}

fn fully_qualified_path<T: Item>(items: &ItemMap, item: &T) -> anyhow::Result<String> {
    let data = item.data();
    let mut path = vec![data.id.to_string()];
    let mut parent_id = data.parent_id;
    while let Some(this_parent_id) = parent_id {
        let parent = items.get(this_parent_id)?;
        let id = parent.data().id.to_string();
        if !id.is_empty() {
            path.push(id);
        }
        parent_id = parent.data().parent_id;
    }
    path.reverse();
    Ok(format!(
        "{}:{}{}",
        T::TYPE.to_string().to_lowercase(),
        path.join("/"),
        if data.is_ambient { " [A]" } else { "" }
    ))
}

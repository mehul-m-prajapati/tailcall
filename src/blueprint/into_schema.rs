use std::borrow::Cow;
use std::sync::Arc;

use async_graphql::dynamic::{
  FieldFuture, FieldValue, SchemaBuilder, {self},
};
use async_graphql_value::ConstValue;

use crate::blueprint::{Blueprint, Definition, Type};
use crate::http::RequestContext;
use crate::lambda::EvaluationContext;

fn to_type_ref(type_of: &Type) -> dynamic::TypeRef {
  match type_of {
    Type::NamedType { name, non_null } => {
      if *non_null {
        dynamic::TypeRef::NonNull(Box::from(dynamic::TypeRef::Named(Cow::Owned(name.clone()))))
      } else {
        dynamic::TypeRef::Named(Cow::Owned(name.clone()))
      }
    }
    Type::ListType { of_type, non_null } => {
      let inner = Box::new(to_type_ref(of_type));
      if *non_null {
        dynamic::TypeRef::NonNull(Box::from(dynamic::TypeRef::List(inner)))
      } else {
        dynamic::TypeRef::List(inner)
      }
    }
  }
}

fn to_type(def: &Definition) -> dynamic::Type {
  match def {
    Definition::ObjectTypeDefinition(def) => {
      let mut object = dynamic::Object::new(def.name.clone());
      for field in def.fields.iter() {
        let field = field.clone();
        let type_ref = to_type_ref(&field.of_type);
        let field_name = &field.name.clone();
        let mut dyn_schema_field = dynamic::Field::new(field_name, type_ref, move |ctx| {
          let req_ctx = ctx.ctx.data::<Arc<RequestContext>>().unwrap();
          let field_name = field.name.clone();
          let resolver = field.resolver.clone();
          FieldFuture::new(async move {
            match resolver {
              None => {
                let ctx = EvaluationContext::new(req_ctx, &ctx);
                Ok(ctx.path_value(&[field_name]).map(|a| FieldValue::from(a.to_owned())))
              }
              Some(expr) => {
                let ctx = EvaluationContext::new(req_ctx, &ctx);
                let const_value = expr.eval(&ctx).await?;
                let p = match const_value {
                  ConstValue::List(a) => FieldValue::list(a),
                  a => FieldValue::from(a),
                };
                Ok(Some(p))
              }
            }
          })
        });
        if let Some(description) = &field.description {
          dyn_schema_field = dyn_schema_field.description(description);
        }
        for arg in field.args.iter() {
          dyn_schema_field =
            dyn_schema_field.argument(dynamic::InputValue::new(arg.name.clone(), to_type_ref(&arg.of_type)));
        }
        object = object.field(dyn_schema_field);
      }
      for interface in def.implements.iter() {
        object = object.implement(interface.clone());
      }

      dynamic::Type::Object(object)
    }
    Definition::InterfaceTypeDefinition(def) => {
      let mut interface = dynamic::Interface::new(def.name.clone());
      for field in def.fields.iter() {
        interface = interface.field(dynamic::InterfaceField::new(
          field.name.clone(),
          to_type_ref(&field.of_type),
        ));
      }

      dynamic::Type::Interface(interface)
    }
    Definition::InputObjectTypeDefinition(def) => {
      let mut input_object = dynamic::InputObject::new(def.name.clone());
      for field in def.fields.iter() {
        input_object = input_object.field(dynamic::InputValue::new(
          field.name.clone(),
          to_type_ref(&field.of_type),
        ));
      }

      dynamic::Type::InputObject(input_object)
    }
    Definition::ScalarTypeDefinition(def) => {
      let mut scalar = dynamic::Scalar::new(def.name.clone());
      if let Some(description) = &def.description {
        scalar = scalar.description(description);
      }
      dynamic::Type::Scalar(scalar)
    }
    Definition::EnumTypeDefinition(def) => {
      let mut enum_type = dynamic::Enum::new(def.name.clone());
      for value in def.enum_values.iter() {
        enum_type = enum_type.item(dynamic::EnumItem::new(value.name.clone()));
      }
      dynamic::Type::Enum(enum_type)
    }
    Definition::UnionTypeDefinition(def) => {
      let mut union = dynamic::Union::new(def.name.clone());
      for type_ in def.types.iter() {
        union = union.possible_type(type_.clone());
      }
      dynamic::Type::Union(union)
    }
  }
}

fn create(blueprint: &Blueprint) -> SchemaBuilder {
  let query = blueprint.query();
  let mutation = blueprint.mutation();
  let mut schema = dynamic::Schema::build(query.as_str(), mutation.as_deref(), None);

  for def in blueprint.definitions.iter() {
    schema = schema.register(to_type(def));
  }

  let entity_resolvers = blueprint.entity_resolvers.clone();
  if !entity_resolvers.is_empty() {
    println!("Creating entity resolvers");
    schema = schema.enable_federation();
    schema = schema.entity_resolver(
      move |ctx| {
        let req_ctx = ctx.ctx.data::<Arc<RequestContext>>().unwrap();
        let entity_resolvers = entity_resolvers.clone();
        FieldFuture::new(async move {
          let representations = ctx.args.try_get("representations")?.list()?;
          let mut values = Vec::new();

          for item in representations.iter() {
            let item = item.object()?;
            let typename = item
                .try_get("__typename")
                .and_then(|value| value.string())?;
            let resolver = entity_resolvers.get(typename);
            match resolver {
              None => {},
              Some(None) => {},
              Some(Some(expr)) => {
                let ctx = EvaluationContext::new(req_ctx, &ctx);
                let const_value = expr.eval(&ctx).await?;
                let p = match const_value {
                  ConstValue::List(a) => FieldValue::list(a),
                  a => FieldValue::from(a),
                };
                values.push(p);
              }
            }
          }
          Ok(Some(values))
        })
      }
    );
    
  }
  
  schema
}

impl From<&Blueprint> for SchemaBuilder {
  fn from(blueprint: &Blueprint) -> Self {
    create(blueprint)
  }
}

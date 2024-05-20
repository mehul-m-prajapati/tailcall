use std::collections::BTreeSet;

use anyhow::{bail, Result};
use derive_setters::Setters;
use prost_reflect::prost_types::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorSet, ServiceDescriptorProto,
};

use super::graphql_type::{GraphQLType, Unparsed};
use crate::core::config::{Arg, Config, Enum, Field, Grpc, Tag, Type, Union};

/// Assists in the mapping and retrieval of proto type names to custom formatted
/// strings based on the descriptor type.
#[derive(Setters)]
struct Context {
    /// The current proto package name.
    namespace: Vec<String>,

    /// Final configuration that's being built up.
    config: Config,

    /// Root GraphQL query type
    query: String,
}

impl Context {
    fn new(query: &str) -> Self {
        Self {
            query: query.to_string(),
            namespace: Default::default(),
            config: Default::default(),
        }
    }

    /// Resolves the actual name and inserts the type.
    fn insert_type(mut self, name: String, ty: Type) -> Self {
        self.config.types.insert(name.to_string(), ty);
        self
    }

    /// Converts oneof definitions in message to set of types with union
    fn insert_oneofs(
        mut self,
        type_name: String, // name of the message
        base_type: Type,   // that's the type with fields that are not oneofs
        oneof_fields: Vec<Vec<(String, Field)>>,
    ) -> Self {
        fn collect_types(
            type_name: String,
            base_type: Type,
            oneof_fields: &[Vec<(String, Field)>],
            output: &mut Vec<(String, Type)>,
        ) {
            let Some(one_of) = oneof_fields.first() else {
                output.push((type_name, base_type));

                return;
            };

            let oneof_fields = &oneof_fields[1..];

            // since all of the fields are actually optional in protobuf generate also a
            // type without this oneof completely
            collect_types(
                format!("{type_name}__Var"),
                base_type.clone(),
                oneof_fields,
                output,
            );

            for (i, (field_name, field)) in one_of.iter().enumerate() {
                let mut new_type = base_type.clone();
                let mut field = field.clone();

                // mark this field as required to force type-check on specific variant of oneof
                field.required = true;

                // add new field specific to this variant of oneof field
                new_type.fields.insert(field_name.clone(), field);

                collect_types(
                    format!("{type_name}__Var{i}"),
                    new_type,
                    oneof_fields,
                    output,
                );
            }
        }

        let mut union_types = Vec::new();

        collect_types(
            type_name.clone(),
            base_type,
            &oneof_fields,
            &mut union_types,
        );

        let mut union_ = Union::default();

        for (type_name, ty) in union_types {
            union_.types.insert(type_name.clone());

            self = self.insert_type(type_name, ty);
        }

        self.config.unions.insert(type_name, union_);

        self
    }

    /// Processes proto enum types.
    fn append_enums(mut self, enums: &Vec<EnumDescriptorProto>) -> Self {
        for enum_ in enums {
            let enum_name = enum_.name();

            let variants = enum_
                .value
                .iter()
                .map(|v| GraphQLType::new(v.name()).into_enum_variant().to_string())
                .collect::<BTreeSet<String>>();

            let type_name = GraphQLType::new(enum_name)
                .extend(self.namespace.as_slice())
                .into_enum()
                .to_string();
            self.config
                .enums
                .insert(type_name, Enum { variants, doc: None });
        }
        self
    }

    /// Processes proto message types.
    fn append_msg_type(mut self, messages: &Vec<DescriptorProto>) -> Result<Self> {
        for message in messages {
            let msg_name = message.name();
            if let Some(options) = message.options.as_ref() {
                if options.map_entry.unwrap_or_default() {
                    continue;
                }
            }

            // first append the name of current message as namespace
            self.namespace.push(msg_name.to_string());
            self = self.append_enums(&message.enum_type);
            self = self.append_msg_type(&message.nested_type)?;
            // then drop it after handling nested types
            self.namespace.pop();

            let mut oneof_fields: Vec<_> = message.oneof_decl.iter().map(|_| Vec::new()).collect();

            let msg_type = GraphQLType::new(msg_name)
                .extend(self.namespace.as_slice())
                .into_object_type();

            let mut ty = Type::default();
            for field in message.field.iter() {
                let field_name = GraphQLType::new(field.name())
                    .extend(self.namespace.as_slice())
                    .into_field();

                let mut cfg_field = Field::default();

                let label = field.label().as_str_name().to_lowercase();
                cfg_field.list = label.contains("repeated");
                cfg_field.required = label.contains("required") || cfg_field.list;

                if field.type_name.is_some() {
                    // for non-primitive types
                    let type_of = graphql_type_from_ref(field.type_name())?
                        .into_object_type()
                        .to_string();

                    cfg_field.type_of = type_of;
                } else {
                    let type_of = convert_primitive_type(field.r#type().as_str_name());
                    if type_of.eq("JSON") {
                        cfg_field.list = false;
                        cfg_field.required = false;
                    }
                    cfg_field.type_of = type_of;
                }

                if let Some(oneof_index) = field.oneof_index {
                    oneof_fields[oneof_index as usize].push((field_name.to_string(), cfg_field));
                } else {
                    ty.fields.insert(field_name.to_string(), cfg_field);
                }
            }

            ty.tag = Some(Tag { id: msg_type.id() });

            if message.oneof_decl.is_empty() {
                self = self.insert_type(msg_type.to_string(), ty);
            } else {
                self = self.insert_oneofs(msg_type.to_string(), ty, oneof_fields);
            }
        }
        Ok(self)
    }

    /// Processes proto service definitions and their methods.
    fn append_query_service(mut self, services: &Vec<ServiceDescriptorProto>) -> Result<Self> {
        if services.is_empty() {
            return Ok(self);
        }

        for service in services {
            let service_name = service.name();
            for method in &service.method {
                let field_name = GraphQLType::new(method.name())
                    .extend(self.namespace.as_slice())
                    .push(service_name)
                    .into_method();

                let mut cfg_field = Field::default();
                let mut body = None;

                if let Some(graphql_type) = get_input_type(method.input_type())? {
                    let key = graphql_type.clone().into_field().to_string();
                    let type_of = graphql_type.into_object_type().to_string();
                    let val = Arg {
                        type_of,
                        list: false,
                        required: true,
                        /* Setting it not null by default. There's no way to infer this
                         * from proto file */
                        doc: None,
                        modify: None,
                        default_value: None,
                    };

                    body = Some(format!("{{{{.args.{key}}}}}"));
                    cfg_field.args.insert(key, val);
                }

                let output_ty = get_output_type(method.output_type())?
                    .into_object_type()
                    .to_string();
                cfg_field.type_of = output_ty;
                cfg_field.required = true;

                cfg_field.grpc = Some(Grpc {
                    base_url: None,
                    body,
                    group_by: vec![],
                    headers: vec![],
                    method: field_name.id(),
                });

                let ty = self
                    .config
                    .types
                    .entry(self.query.clone())
                    .or_insert_with(|| {
                        self.config.schema.query = Some(self.query.clone());
                        Type::default()
                    });

                ty.fields.insert(field_name.to_string(), cfg_field);
            }
        }
        Ok(self)
    }
}

fn graphql_type_from_ref(name: &str) -> Result<GraphQLType<Unparsed>> {
    if !name.starts_with('.') {
        bail!("Expected fully-qualified name for reference type but got {name}. This is a bug!");
    }

    let name = &name[1..];

    if let Some((package, name)) = name.rsplit_once('.') {
        Ok(GraphQLType::new(name).push(package))
    } else {
        Ok(GraphQLType::new(name))
    }
}

/// Converts proto field types to a custom format.
fn convert_primitive_type(proto_ty: &str) -> String {
    let binding = proto_ty.to_lowercase();
    let proto_ty = binding.strip_prefix("type_").unwrap_or(proto_ty);
    match proto_ty {
        "double" | "float" => "Float",
        "int32" | "int64" | "fixed32" | "fixed64" | "uint32" | "uint64" => "Int",
        "bool" => "Boolean",
        "string" | "bytes" => "String",
        "message" => "JSON", // JSON scalar is preloaded by tailcall, so there is no need to
        // explicitly define it in the config.
        x => x,
    }
    .to_string()
}

/// Determines the output type for a service method.
fn get_output_type(output_ty: &str) -> Result<GraphQLType<Unparsed>> {
    // type, required
    match output_ty {
        ".google.protobuf.Empty" => {
            // If it's no response is expected, we return an Empty scalar type
            Ok(GraphQLType::new("Empty"))
        }
        _ => {
            // Setting it not null by default. There's no way to infer this from proto file
            graphql_type_from_ref(output_ty)
        }
    }
}

fn get_input_type(input_ty: &str) -> Result<Option<GraphQLType<Unparsed>>> {
    match input_ty {
        ".google.protobuf.Empty" | "" => Ok(None),
        _ => graphql_type_from_ref(input_ty).map(Some),
    }
}

/// The main entry point that builds a Config object from proto descriptor sets.
pub fn from_proto(descriptor_sets: &[FileDescriptorSet], query: &str) -> Result<Config> {
    let mut ctx = Context::new(query);
    for descriptor_set in descriptor_sets.iter() {
        for file_descriptor in descriptor_set.file.iter() {
            ctx.namespace = vec![file_descriptor.package().to_string()];

            ctx = ctx
                .append_enums(&file_descriptor.enum_type)
                .append_msg_type(&file_descriptor.message_type)?
                .append_query_service(&file_descriptor.service)?;
        }
    }

    let unused_types = ctx.config.unused_types();
    ctx.config = ctx.config.remove_types(unused_types);

    Ok(ctx.config)
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use prost_reflect::prost_types::FileDescriptorSet;
    use tailcall_fixtures::protobuf;

    use super::*;
    use crate::core::config::{ConfigModule, Resolution};

    fn compile_protobuf(files: &[&str]) -> Result<FileDescriptorSet> {
        Ok(protox::compile(files, [protobuf::SELF])?)
    }

    macro_rules! assert_gen {
        ($( $set:expr ), +) => {
            let set = compile_protobuf(&[$( $set ),+]).unwrap();
            let config = from_proto(&[set], "Query").unwrap();
            let config_module = ConfigModule::from(config).resolve_ambiguous_types(|v| Resolution {
                input: format!("{}Input", v),
                output: v.to_owned(),
            });
            let result = config_module.to_sdl();
            insta::assert_snapshot!(result);
        };
    }

    #[test]
    fn test_from_proto() {
        // news_enum.proto covers:
        // test for mutation
        // test for empty objects
        // test for optional type
        // test for enum
        // test for repeated fields
        // test for a type used as both input and output
        // test for two types having same name in different packages

        assert_gen!(protobuf::NEWS, protobuf::GREETINGS_A, protobuf::GREETINGS_B);
    }

    #[test]
    fn test_from_proto_no_pkg_file() {
        assert_gen!(protobuf::NEWS_NO_PKG);
    }

    #[test]
    fn test_from_proto_no_service_file() {
        assert_gen!(protobuf::NEWS_NO_SERVICE);
    }

    #[test]
    fn test_greetings_proto_file() {
        assert_gen!(protobuf::GREETINGS, protobuf::GREETINGS_MESSAGE);
    }

    #[test]
    fn test_config_from_sdl() -> Result<()> {
        let set =
            compile_protobuf(&[protobuf::NEWS, protobuf::GREETINGS_A, protobuf::GREETINGS_B])?;

        let set1 = compile_protobuf(&[protobuf::NEWS])?;
        let set2 = compile_protobuf(&[protobuf::GREETINGS_A])?;
        let set3 = compile_protobuf(&[protobuf::GREETINGS_B])?;

        let actual = from_proto(&[set], "Query")?.to_sdl();
        let expected = from_proto(&[set1, set2, set3], "Query")?.to_sdl();

        pretty_assertions::assert_eq!(actual, expected);
        Ok(())
    }

    #[test]
    fn test_required_types() {
        // required fields are deprecated in proto3 (https://protobuf.dev/programming-guides/dos-donts/#add-required)
        // this example uses proto2 to test the same.
        // for proto3 it's guaranteed to have a default value (https://protobuf.dev/programming-guides/proto3/#default)
        // and our implementation (https://github.com/tailcallhq/tailcall/pull/1537) supports default values by default.
        // so we do not need to explicitly mark fields as required.

        assert_gen!(protobuf::PERSON);
    }

    #[test]
    fn test_movies() {
        assert_gen!(protobuf::MOVIES);
    }

    #[test]
    fn test_nested_types() {
        assert_gen!(protobuf::NESTED_TYPES);
    }

    #[test]
    fn test_oneof_types() {
        assert_gen!(protobuf::ONEOF);
    }
}

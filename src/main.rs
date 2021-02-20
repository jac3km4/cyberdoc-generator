use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::rc::Rc;

use gumdrop::Options;
use redscript::bundle::{ConstantPool, PoolIndex, ScriptBundle};
use redscript::definition::{Class, Definition, DefinitionValue, Type};
use redscript::error::Error;
use serde::ser::{Serialize, SerializeStruct, Serializer};
use serde_json::{json, Value};

#[derive(Debug, Options)]
struct AppOpts {
    #[options(required, short = "i", help = "redscript bundle file to read")]
    input: PathBuf,
    #[options(required, short = "o", help = "redscript bundle file to write")]
    output: PathBuf,
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = AppOpts::parse_args_default(&args).unwrap();

    let bundle = ScriptBundle::load(&mut BufReader::new(File::open(opts.input)?))?;
    let pool = &bundle.pool;
    std::fs::create_dir_all(&opts.output)?;

    for (idx, def) in pool.roots().filter(|(_, def)| {
        matches!(&def.value, DefinitionValue::Class(_))
            || matches!(&def.value, DefinitionValue::Function(_))
            || matches!(&def.value, DefinitionValue::Enum(_))
    }) {
        let path = opts.output.as_path().join(format!("{}.json", idx.index));
        let encoded = encode_definition(def, pool)?;
        std::fs::write(path, serde_json::to_string(&encoded).unwrap())?;
    }

    let index_path = opts.output.as_path().join("index.json");
    let index = build_index(pool);
    std::fs::write(index_path, serde_json::to_string(&index).unwrap())?;
    Ok(())
}

pub fn encode_definition(definition: &Definition, pool: &ConstantPool) -> Result<Value, Error> {
    let result = match &definition.value {
        DefinitionValue::Type(type_) => match type_ {
            Type::Prim => json!({"tag": "Type", "kind": "Prim", "name": pool.names.get(definition.name)?.as_ref()}),
            Type::Class => {
                let class = find_type(definition.name, pool).unwrap();
                json!({"tag": "Type", "kind": "Class", "name": pool.names.get(definition.name)?.as_ref(), "index": class.index })
            }
            Type::Ref(inner) => {
                json!({"tag": "Type", "kind": "Ref", "inner": encode_definition(pool.definition(*inner)?, pool)?})
            }
            Type::WeakRef(inner) => {
                json!({"tag": "Type", "kind": "WeakRef", "inner": encode_definition(pool.definition(*inner)?, pool)?})
            }
            Type::ScriptRef(inner) => {
                json!({"tag": "Type", "kind": "ScriptRef", "inner": encode_definition(pool.definition(*inner)?, pool)?})
            }
            Type::Array(inner) => {
                json!({"tag": "Type", "kind": "Array", "inner": encode_definition(pool.definition(*inner)?, pool)?})
            }
            Type::StaticArray(inner, size) => {
                json!({"tag": "Type", "kind": "StaticArray", "size": size, "inner": encode_definition(pool.definition(*inner)?, pool)?})
            }
        },
        DefinitionValue::Class(class) => {
            let fields: Result<Vec<Value>, Error> = class
                .fields
                .iter()
                .map(|f| encode_definition(pool.definition(*f)?, pool))
                .collect();
            let methods: Result<Vec<Value>, Error> = class
                .functions
                .iter()
                .map(|f| encode_definition(pool.definition(*f)?, pool))
                .collect();
            json!({
                "tag": "Class",
                "name": pool.names.get(definition.name)?.as_ref(),
                "visibility": format!("{}", class.visibility).to_lowercase(),
                "bases": collect_bases(class.base, pool)?,
                "fields": fields?,
                "methods": methods?,
                "isNative": class.flags.is_native(),
                "isAbstract": class.flags.is_abstract(),
                "isFinal": class.flags.is_final(),
                "isStruct": class.flags.is_struct(),
            })
        }
        DefinitionValue::EnumValue(val) => json!({
            "tag": "EnumValue",
            "name": pool.names.get(definition.name)?.as_ref(),
            "value": val,
        }),
        DefinitionValue::Enum(enum_) => {
            let members: Result<Vec<Value>, Error> = enum_
                .members
                .iter()
                .map(|m| encode_definition(pool.definition(*m)?, pool))
                .collect();
            json!({
                "tag": "Enum",
                "name": pool.names.get(definition.name)?.as_ref(),
                "members": members?
            })
        }
        DefinitionValue::Function(fun) => {
            let parameters: Result<Vec<Value>, Error> = fun
                .parameters
                .iter()
                .map(|m| encode_definition(pool.definition(*m)?, pool))
                .collect();
            json!({
                "tag": "Function",
                "name": pool.names.get(definition.name)?.as_ref(),
                "parameters": parameters?,
                "returnType": fun.return_type.map(|idx| encode_definition(pool.definition(idx).unwrap(), pool).unwrap()),
                "visibility": format!("{}", fun.visibility).to_lowercase(),
                "isStatic": fun.flags.is_static(),
                "isFinal": fun.flags.is_final(),
                "isExec": fun.flags.is_exec(),
                "isCallback": fun.flags.is_callback(),
                "isNative": fun.flags.is_native(),
                "source": fun.source.as_ref().map(|idx| encode_definition(pool.definition(idx.file).unwrap(), pool).unwrap())
            })
        }
        DefinitionValue::Parameter(param) => json!({
            "tag": "Parameter",
            "name": pool.names.get(definition.name)?.as_ref(),
            "type": encode_definition(pool.definition(param.type_)?, pool)?,
            "isOut": param.flags.is_out(),
            "isOptional": param.flags.is_optional(),
        }),
        DefinitionValue::Field(field) => json!({
            "tag": "Field",
            "name": pool.names.get(definition.name)?.as_ref(),
            "type": encode_definition(pool.definition(field.type_)?, pool)?,
            "isNative": field.flags.is_native(),
            "isEdit": field.flags.is_edit(),
            "isInline": field.flags.is_inline(),
            "isConst": field.flags.is_const(),
            "isRep": field.flags.is_rep(),
            "isPersistent": field.flags.is_persistent(),
        }),
        DefinitionValue::SourceFile(f) => Value::String(f.path.display().to_string()),
        DefinitionValue::Local(_) => panic!(),
    };
    Ok(result)
}

fn find_type(name: PoolIndex<String>, pool: &ConstantPool) -> Option<PoolIndex<Class>> {
    pool.definitions().find_map(|(idx, def)| match &def.value {
        DefinitionValue::Class(_) if def.name == name => Some(idx.cast()),
        DefinitionValue::Enum(_) if def.name == name => Some(idx.cast()),
        _ => None,
    })
}

fn build_index(pool: &ConstantPool) -> Vec<Reference> {
    pool.roots()
        .filter(|(_, def)| {
            matches!(&def.value, DefinitionValue::Class(_))
                || matches!(&def.value, DefinitionValue::Function(_))
                || matches!(&def.value, DefinitionValue::Enum(_))
        })
        .map(|(index, def)| {
            let name = pool.names.get(def.name).unwrap();
            let pretty = Rc::new(name.split(';').next().unwrap().to_string());
            Reference { name: pretty, index }
        })
        .collect()
}

fn collect_bases(idx: PoolIndex<Class>, pool: &ConstantPool) -> Result<Vec<Reference>, Error> {
    let mut bases = vec![];
    if idx != PoolIndex::UNDEFINED {
        let reference = Reference {
            name: pool.definition_name(idx)?,
            index: idx.cast(),
        };
        let class = pool.class(idx)?;
        bases.push(reference);
        bases.append(&mut collect_bases(class.base, pool)?);
    }
    Ok(bases)
}

pub struct Reference {
    name: Rc<String>,
    index: PoolIndex<Definition>,
}

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Reference", 2)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("index", &self.index.index)?;
        state.end()
    }
}

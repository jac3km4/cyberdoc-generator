use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::rc::Rc;

use gumdrop::Options;
use redscript::bundle::{CName, ConstantPool, PoolIndex, ScriptBundle};
use redscript::definition::{AnyDefinition, Class, Definition, Type};
use serde::ser::{Serialize, SerializeStruct, Serializer};
use serde_json::{json, Value};

#[derive(Debug, Options)]
struct AppOpts {
    #[options(required, short = "i", help = "redscript bundle file to read")]
    input: PathBuf,
    #[options(required, short = "o", help = "output directory")]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = AppOpts::parse_args_default(&args)?;

    let bundle = ScriptBundle::load(&mut BufReader::new(File::open(opts.input)?))?;
    let pool = &bundle.pool;
    std::fs::create_dir_all(&opts.output)?;

    for (idx, def) in pool.roots().filter(|(_, def)| {
        matches!(&def.value, AnyDefinition::Class(_))
            || matches!(&def.value, AnyDefinition::Function(_))
            || matches!(&def.value, AnyDefinition::Enum(_))
    }) {
        let idx: u32 = idx.into();
        let path = opts.output.as_path().join(format!("{}.json", idx));
        let encoded = encode_definition(def, pool)?;
        std::fs::write(path, serde_json::to_string(&encoded)?)?;
    }

    let index_path = opts.output.as_path().join("index.json");
    let index = build_index(pool);
    std::fs::write(index_path, serde_json::to_string(&index)?)?;
    Ok(())
}

pub fn encode_definition(definition: &Definition, pool: &ConstantPool) -> Result<Value, Box<dyn Error>> {
    let result = match &definition.value {
        AnyDefinition::Type(type_) => match type_ {
            Type::Prim => json!({"tag": "Type", "kind": "Prim", "name": pool.names.get(definition.name)?.as_ref()}),
            Type::Class => {
                let class = find_type(definition.name, pool).unwrap();
                let class_idx: u32 = class.into();
                json!({"tag": "Type", "kind": "Class", "name": pool.names.get(definition.name)?.as_ref(), "index": class_idx })
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
        AnyDefinition::Class(class) => {
            let fields: Result<Vec<Value>, Box<dyn Error>> = class
                .fields
                .iter()
                .map(|f| encode_definition(pool.definition(*f)?, pool))
                .collect();
            let methods: Result<Vec<Value>, Box<dyn Error>> = class
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
        AnyDefinition::EnumValue(val) => json!({
            "tag": "EnumValue",
            "name": pool.names.get(definition.name)?.as_ref(),
            "value": val,
        }),
        AnyDefinition::Enum(enum_) => {
            let members: Result<Vec<Value>, Box<dyn Error>> = enum_
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
        AnyDefinition::Function(fun) => {
            let parameters: Result<Vec<Value>, Box<dyn Error>> = fun
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
        AnyDefinition::Parameter(param) => json!({
            "tag": "Parameter",
            "name": pool.names.get(definition.name)?.as_ref(),
            "type": encode_definition(pool.definition(param.type_)?, pool)?,
            "isOut": param.flags.is_out(),
            "isOptional": param.flags.is_optional(),
        }),
        AnyDefinition::Field(field) => json!({
            "tag": "Field",
            "name": pool.names.get(definition.name)?.as_ref(),
            "type": encode_definition(pool.definition(field.type_)?, pool)?,
            "isNative": field.flags.is_native(),
            "isEdit": field.flags.is_editable(),
            "isInline": field.flags.is_inline(),
            "isConst": field.flags.is_const(),
            "isRep": field.flags.is_replicated(),
            "isPersistent": field.flags.is_persistent(),
        }),
        AnyDefinition::SourceFile(f) => Value::String(f.path.display().to_string()),
        AnyDefinition::Local(_) => panic!(),
    };
    Ok(result)
}

fn find_type(name: PoolIndex<CName>, pool: &ConstantPool) -> Option<PoolIndex<Class>> {
    pool.definitions().find_map(|(idx, def)| match &def.value {
        AnyDefinition::Class(_) if def.name == name => Some(idx.cast()),
        AnyDefinition::Enum(_) if def.name == name => Some(idx.cast()),
        _ => None,
    })
}

fn build_index(pool: &ConstantPool) -> Vec<Reference> {
    pool.roots()
        .filter(|(_, def)| {
            matches!(&def.value, AnyDefinition::Class(_))
                || matches!(&def.value, AnyDefinition::Function(_))
                || matches!(&def.value, AnyDefinition::Enum(_))
        })
        .map(|(index, def)| {
            let name = pool.names.get(def.name).unwrap();
            let pretty = Rc::from(name.split(';').next().unwrap());
            let base = def.value.as_class().map(|c| c.base.cast());
            Reference {
                name: pretty,
                index,
                base,
            }
        })
        .collect()
}

fn collect_bases(idx: PoolIndex<Class>, pool: &ConstantPool) -> Result<Vec<Reference>, Box<dyn Error>> {
    let mut bases = vec![];
    if idx != PoolIndex::UNDEFINED {
        let reference = Reference {
            name: pool.def_name(idx)?,
            index: idx.cast(),
            base: None,
        };
        let class = pool.class(idx)?;
        bases.push(reference);
        bases.append(&mut collect_bases(class.base, pool)?);
    }
    Ok(bases)
}

pub struct Reference {
    name: Rc<str>,
    index: PoolIndex<Definition>,
    base: Option<PoolIndex<Definition>>,
}

impl Serialize for Reference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Reference", 3)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("index", &u32::from(self.index))?;
        state.serialize_field("base", &self.base.map(u32::from))?;
        state.end()
    }
}

use std::{collections::HashMap, sync::Mutex};

use abi_stable::std_types::{ROption, RStr, RString, RVec};
use monty_extension_api::{
    ExtArgs, ExtContext, ExtError, ExtHandle, ExtKeyValue, ExtValue, MontyExtension, ResourceBudget, monty_extension,
    monty_handles,
};

#[derive(Clone)]
struct Record {
    value: i64,
}

#[monty_handles(extension = SampleExtension, module = "sample")]
enum StoredObject {
    Record(Record),
}

struct SampleExtension {
    objects: Mutex<HashMap<u64, StoredObject>>,
    next_id: Mutex<u64>,
}

#[monty_extension(
    name = "sample",
    version = "0.1.0",
    skill = "# sample",
    stubs = "def make_record(value: int) -> Record: ..."
)]
impl SampleExtension {
    fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    #[function()]
    fn make_record(&self, value: i64) -> RecordHandle {
        self.store_record(Record { value })
    }

    #[function()]
    fn greet(&self, name: &str, excited: Option<bool>) -> String {
        let _ = &self.objects;
        let suffix = if excited.unwrap_or(false) { "!" } else { "." };
        format!("hello {name}{suffix}")
    }

    #[method(name = "value")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "typed handles are generated as owned wrappers"
    )]
    fn value_method(&self, record: RecordHandle) -> Result<i64, ExtError> {
        self.with_record(&record, "value", |record| Ok(record.value))
    }

    #[shutdown()]
    fn shutdown_extension(&self) {
        self.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

fn empty_context() -> ExtContext {
    ExtContext {
        budget: ResourceBudget {
            remaining_time_ms: ROption::RNone,
            remaining_allocations: ROption::RNone,
        },
    }
}

#[test]
fn macro_generated_manifest_and_dispatch_work() {
    let extension = SampleExtension::new();
    let manifest = extension.manifest();

    assert_eq!(manifest.module_name.as_str(), "sample");
    assert_eq!(manifest.version.as_str(), "0.1.0");
    assert_eq!(manifest.functions.len(), 2);
    assert_eq!(manifest.functions[0].name.as_str(), "make_record");
    assert_eq!(manifest.functions[1].name.as_str(), "greet");
}

#[test]
fn macro_generated_optional_keyword_arguments_work() {
    let extension = SampleExtension::new();
    let result = extension.call(
        RStr::from("greet"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("Ada"))]),
            keyword: RVec::from(vec![ExtKeyValue {
                key: RString::from("excited"),
                value: ExtValue::Bool(true),
            }]),
        },
        &empty_context(),
    );

    match result {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(value)) => {
            assert_eq!(value.as_str(), "hello Ada!");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
        abi_stable::std_types::RResult::RErr(error) => {
            panic!("unexpected error: {error:?}");
        }
    }
}

#[test]
fn macro_generated_handle_methods_work() {
    let extension = SampleExtension::new();
    let created = extension.call(
        RStr::from("make_record"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Int(41)]),
            keyword: RVec::new(),
        },
        &empty_context(),
    );

    let handle = match created {
        abi_stable::std_types::RResult::ROk(ExtValue::Handle(handle)) => handle,
        other => panic!("unexpected make_record result: {other:?}"),
    };

    let value = extension.call_method(
        &handle,
        RStr::from("value"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &empty_context(),
    );

    match value {
        abi_stable::std_types::RResult::ROk(ExtValue::Int(value)) => assert_eq!(value, 41),
        other => panic!("unexpected method result: {other:?}"),
    }
}

#[test]
fn macro_generated_errors_preserve_python_style_messages() {
    let extension = SampleExtension::new();
    let result = extension.call(
        RStr::from("greet"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Int(1)]),
            keyword: RVec::new(),
        },
        &empty_context(),
    );

    match result {
        abi_stable::std_types::RResult::RErr(error) => {
            assert_eq!(error.exception_type.as_str(), "TypeError");
            assert_eq!(error.message.as_str(), "greet() argument 'name' must be a str");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
    }
}

#[test]
fn macro_generated_unknown_methods_use_attribute_error() {
    let extension = SampleExtension::new();
    let handle = ExtHandle {
        type_name: RString::from("sample.Record"),
        handle_id: 999,
        extension_id: RString::from("sample"),
    };

    let result = extension.call_method(
        &handle,
        RStr::from("missing"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &empty_context(),
    );

    match result {
        abi_stable::std_types::RResult::RErr(error) => {
            assert_eq!(error.exception_type.as_str(), "AttributeError");
            assert_eq!(error.message.as_str(), "'Record' object has no attribute 'missing'");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
    }
}

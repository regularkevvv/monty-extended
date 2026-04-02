//! Smoke tests for the module-level `#[monty_module] mod` API.
//!
//! Validates that `#[monty_class]`, `#[monty_function]`, `#[monty_methods]`,
//! and `#[monty_shutdown]` inside a `mod` block produce the same runtime
//! behaviour as the classic impl-level macros.

use abi_stable::std_types::{ROption, RStr, RString, RVec};
use modtest_ext::Extension;
use monty_extension_api::{
    ExtArgs, ExtContext, ExtError, ExtHandle, ExtKeyValue, ExtValue, MontyExtension, ResourceBudget, monty_module,
};

#[monty_module(
    name = "modtest",
    version = "0.3.0",
    skill = "# modtest",
    stubs = "def make_item(label: str) -> Item: ..."
)]
mod modtest_ext {
    use super::*;

    /// A simple stored item with a label and count.
    #[monty_class]
    struct Item {
        label: String,
        count: i64,
    }

    /// A second class to test multi-class support.
    #[monty_class]
    struct Counter {
        value: i64,
    }

    #[monty_function()]
    fn make_item(ext: &Extension, label: &str, count: Option<i64>) -> ItemHandle {
        ext.store_item(Item {
            label: label.to_string(),
            count: count.unwrap_or(0),
        })
    }

    #[monty_function(name = "greet")]
    fn greet_fn(ext: &Extension, name: &str) -> String {
        let _ = ext;
        format!("hello {name}")
    }

    #[monty_function()]
    fn make_counter(ext: &Extension, initial: i64) -> CounterHandle {
        ext.store_counter(Counter { value: initial })
    }

    #[monty_methods]
    impl Item {
        #[expect(clippy::needless_pass_by_value, reason = "handle wrappers are owned")]
        fn label(ext: &Extension, item: ItemHandle) -> Result<String, ExtError> {
            ext.with_item(&item, "label", |i| Ok(i.label.clone()))
        }

        #[expect(clippy::needless_pass_by_value, reason = "handle wrappers are owned")]
        fn count(ext: &Extension, item: ItemHandle) -> Result<i64, ExtError> {
            ext.with_item(&item, "count", |i| Ok(i.count))
        }

        #[expect(clippy::needless_pass_by_value, reason = "handle wrappers are owned")]
        #[monty_method(name = "display")]
        fn display_method(ext: &Extension, item: ItemHandle) -> Result<String, ExtError> {
            ext.with_item(&item, "display", |i| Ok(format!("{}({})", i.label, i.count)))
        }
    }

    #[monty_methods]
    impl Counter {
        #[expect(clippy::needless_pass_by_value, reason = "handle wrappers are owned")]
        fn value(ext: &Extension, counter: CounterHandle) -> Result<i64, ExtError> {
            ext.with_counter(&counter, "value", |c| Ok(c.value))
        }

        #[expect(clippy::needless_pass_by_value, reason = "handle wrappers are owned")]
        fn increment(ext: &Extension, counter: CounterHandle) -> Result<(), ExtError> {
            ext.with_counter_mut(&counter, "increment", |c| {
                c.value += 1;
                Ok(())
            })
        }
    }

    #[monty_shutdown()]
    fn shutdown(ext: &Extension) {
        ext.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

fn ctx() -> ExtContext {
    ExtContext {
        budget: ResourceBudget {
            remaining_time_ms: ROption::RNone,
            remaining_allocations: ROption::RNone,
        },
    }
}

#[test]
fn module_level_manifest_is_correct() {
    let ext = Extension::new();
    let manifest = ext.manifest();

    assert_eq!(manifest.module_name.as_str(), "modtest");
    assert_eq!(manifest.version.as_str(), "0.3.0");
    assert_eq!(manifest.functions.len(), 3);
    assert_eq!(manifest.functions[0].name.as_str(), "make_item");
    assert_eq!(manifest.functions[1].name.as_str(), "greet");
    assert_eq!(manifest.functions[2].name.as_str(), "make_counter");
}

#[test]
fn module_level_function_dispatch_works() {
    let ext = Extension::new();
    let result = ext.call(
        RStr::from("greet"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("world"))]),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match result {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(value)) => {
            assert_eq!(value.as_str(), "hello world");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn module_level_optional_arguments_work() {
    let ext = Extension::new();

    let result = ext.call(
        RStr::from("make_item"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("x"))]),
            keyword: RVec::from(vec![ExtKeyValue {
                key: RString::from("count"),
                value: ExtValue::Int(42),
            }]),
        },
        &ctx(),
    );

    let handle = match result {
        abi_stable::std_types::RResult::ROk(ExtValue::Handle(h)) => h,
        other => panic!("unexpected make_item result: {other:?}"),
    };

    let count = ext.call_method(
        &handle,
        RStr::from("count"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match count {
        abi_stable::std_types::RResult::ROk(ExtValue::Int(v)) => assert_eq!(v, 42),
        other => panic!("unexpected count result: {other:?}"),
    }
}

#[test]
fn module_level_handle_methods_work() {
    let ext = Extension::new();
    let created = ext.call(
        RStr::from("make_item"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("widget")), ExtValue::Int(5)]),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    let handle = match created {
        abi_stable::std_types::RResult::ROk(ExtValue::Handle(h)) => h,
        other => panic!("unexpected make_item result: {other:?}"),
    };

    let label = ext.call_method(
        &handle,
        RStr::from("label"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match label {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(s)) => {
            assert_eq!(s.as_str(), "widget");
        }
        other => panic!("unexpected label result: {other:?}"),
    }

    let display = ext.call_method(
        &handle,
        RStr::from("display"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match display {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(s)) => {
            assert_eq!(s.as_str(), "widget(5)");
        }
        other => panic!("unexpected display result: {other:?}"),
    }
}

#[test]
fn module_level_multiple_classes_work() {
    let ext = Extension::new();

    let counter = ext.call(
        RStr::from("make_counter"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Int(10)]),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    let handle = match counter {
        abi_stable::std_types::RResult::ROk(ExtValue::Handle(h)) => h,
        other => panic!("unexpected make_counter result: {other:?}"),
    };

    let value = ext.call_method(
        &handle,
        RStr::from("value"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match value {
        abi_stable::std_types::RResult::ROk(ExtValue::Int(v)) => assert_eq!(v, 10),
        other => panic!("unexpected value result: {other:?}"),
    }

    ext.call_method(
        &handle,
        RStr::from("increment"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    let value = ext.call_method(
        &handle,
        RStr::from("value"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match value {
        abi_stable::std_types::RResult::ROk(ExtValue::Int(v)) => assert_eq!(v, 11),
        other => panic!("unexpected value after increment: {other:?}"),
    }
}

#[test]
fn module_level_unknown_function_error() {
    let ext = Extension::new();
    let result = ext.call(
        RStr::from("nonexistent"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match result {
        abi_stable::std_types::RResult::RErr(error) => {
            assert_eq!(error.exception_type.as_str(), "AttributeError");
            assert_eq!(error.message.as_str(), "module 'modtest' has no function 'nonexistent'");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
    }
}

#[test]
fn module_level_unknown_method_error() {
    let ext = Extension::new();
    let handle = ExtHandle {
        type_name: RString::from("modtest.Item"),
        handle_id: 999,
        extension_id: RString::from("modtest"),
    };

    let result = ext.call_method(
        &handle,
        RStr::from("missing"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match result {
        abi_stable::std_types::RResult::RErr(error) => {
            assert_eq!(error.exception_type.as_str(), "AttributeError");
            assert_eq!(error.message.as_str(), "'Item' object has no attribute 'missing'");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
    }
}

#[test]
fn module_level_unknown_type_error() {
    let ext = Extension::new();
    let handle = ExtHandle {
        type_name: RString::from("modtest.Unknown"),
        handle_id: 1,
        extension_id: RString::from("modtest"),
    };

    let result = ext.call_method(
        &handle,
        RStr::from("anything"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match result {
        abi_stable::std_types::RResult::RErr(error) => {
            assert_eq!(error.exception_type.as_str(), "AttributeError");
            assert_eq!(error.message.as_str(), "'Unknown' object has no attribute 'anything'");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => {
            panic!("unexpected result: {other:?}");
        }
    }
}

#[test]
fn module_level_shutdown_clears_objects() {
    let ext = Extension::new();
    ext.call(
        RStr::from("make_item"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("a"))]),
            keyword: RVec::new(),
        },
        &ctx(),
    );
    ext.call(
        RStr::from("make_counter"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Int(0)]),
            keyword: RVec::new(),
        },
        &ctx(),
    );
    assert_eq!(ext.objects.lock().unwrap().len(), 2);
    ext.shutdown();
    assert_eq!(ext.objects.lock().unwrap().len(), 0);
}

#[test]
fn module_level_type_error_on_wrong_argument() {
    let ext = Extension::new();
    let result = ext.call(
        RStr::from("greet"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Int(42)]),
            keyword: RVec::new(),
        },
        &ctx(),
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

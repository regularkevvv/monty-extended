//! Mirror of `proc_macro_smoke.rs` using the ergonomic macro names:
//! `monty_module`, `monty_classes`, `monty_function`, `monty_method`,
//! `monty_shutdown`.  Proves that the new vocabulary compiles identically
//! and produces the same runtime behaviour as the classic names.

use std::{collections::HashMap, sync::Mutex};

use abi_stable::std_types::{ROption, RStr, RString, RVec};
use monty_extension_api::{
    ExtArgs, ExtContext, ExtError, ExtHandle, ExtValue, MontyExtension, ResourceBudget, monty_classes, monty_module,
};

#[derive(Clone)]
struct Widget {
    label: String,
}

#[monty_classes(extension = ErgonomicExtension, module = "ergo")]
enum ErgonomicStored {
    Widget(Widget),
}

struct ErgonomicExtension {
    objects: Mutex<HashMap<u64, ErgonomicStored>>,
    next_id: Mutex<u64>,
}

#[monty_module(
    name = "ergo",
    version = "0.2.0",
    skill = "# ergo",
    stubs = "def make_widget(label: str) -> Widget: ..."
)]
impl ErgonomicExtension {
    fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    #[monty_function()]
    fn make_widget(&self, label: &str) -> WidgetHandle {
        self.store_widget(Widget {
            label: label.to_string(),
        })
    }

    #[monty_function(name = "hello")]
    fn hello_fn(&self, name: &str) -> String {
        let _ = &self.objects;
        format!("hi {name}")
    }

    #[monty_method(name = "label")]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "typed handles are generated as owned wrappers"
    )]
    fn widget_label(&self, widget: WidgetHandle) -> Result<String, ExtError> {
        self.with_widget(&widget, "label", |w| Ok(w.label.clone()))
    }

    #[monty_shutdown()]
    fn cleanup(&self) {
        self.objects
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
fn ergonomic_manifest_and_dispatch_work() {
    let ext = ErgonomicExtension::new();
    let manifest = ext.manifest();

    assert_eq!(manifest.module_name.as_str(), "ergo");
    assert_eq!(manifest.version.as_str(), "0.2.0");
    assert_eq!(manifest.functions.len(), 2);
    assert_eq!(manifest.functions[0].name.as_str(), "make_widget");
    assert_eq!(manifest.functions[1].name.as_str(), "hello");
}

#[test]
fn ergonomic_function_dispatch_works() {
    let ext = ErgonomicExtension::new();
    let result = ext.call(
        RStr::from("hello"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("world"))]),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match result {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(value)) => {
            assert_eq!(value.as_str(), "hi world");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn ergonomic_handle_methods_work() {
    let ext = ErgonomicExtension::new();
    let created = ext.call(
        RStr::from("make_widget"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("btn"))]),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    let handle = match created {
        abi_stable::std_types::RResult::ROk(ExtValue::Handle(h)) => h,
        other => panic!("unexpected make_widget result: {other:?}"),
    };

    let value = ext.call_method(
        &handle,
        RStr::from("label"),
        ExtArgs {
            positional: RVec::new(),
            keyword: RVec::new(),
        },
        &ctx(),
    );

    match value {
        abi_stable::std_types::RResult::ROk(ExtValue::Str(s)) => assert_eq!(s.as_str(), "btn"),
        other => panic!("unexpected method result: {other:?}"),
    }
}

#[test]
fn ergonomic_unknown_method_uses_attribute_error() {
    let ext = ErgonomicExtension::new();
    let handle = ExtHandle {
        type_name: RString::from("ergo.Widget"),
        handle_id: 999,
        extension_id: RString::from("ergo"),
    };

    let result = ext.call_method(
        &handle,
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
            assert_eq!(error.message.as_str(), "'Widget' object has no attribute 'nonexistent'");
        }
        other @ abi_stable::std_types::RResult::ROk(_) => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn ergonomic_shutdown_clears_objects() {
    let ext = ErgonomicExtension::new();
    ext.call(
        RStr::from("make_widget"),
        ExtArgs {
            positional: RVec::from(vec![ExtValue::Str(RString::from("x"))]),
            keyword: RVec::new(),
        },
        &ctx(),
    );
    assert_eq!(ext.objects.lock().unwrap().len(), 1);
    ext.shutdown();
    assert_eq!(ext.objects.lock().unwrap().len(), 0);
}

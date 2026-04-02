//! Extension registry and Value ↔ ExtValue bridge.
//!
//! This module provides the `ExtensionRegistry` which stores loaded extensions (both native
//! and host-backed) and creates importable modules from them. It also contains the bridge
//! functions for converting between Monty's internal `Value` type and the ABI-stable `ExtValue`.
//!
//! # Architecture
//!
//! Extensions are registered before execution begins. The compiler checks the registry during
//! import compilation — if a module name matches a registered extension, it emits
//! `LoadExtensionModule` instead of `RaiseImportError`. At runtime, the VM creates the
//! module on the heap and populates it with `ExtensionFunction` values.
//!
//! Native extension calls go directly through the `MontyExtension` trait object (no suspension).
//! Host extension calls yield `CallResult::External` so the host language can dispatch them.

use std::{collections::HashMap, fmt, mem::size_of};

use abi_stable::std_types::{RBox, ROption, RVec};
use monty_extension_api::{
    ExtArgs, ExtContext, ExtHandle, ExtKeyValue, ExtManifest, ExtResult, ExtValue, MontyExtension_TO, ResourceBudget,
};

use crate::{
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId, HeapItem},
    intern::{InternerBuilder, Interns, StringId},
    resource::ResourceTracker,
    types::{Bytes, Dict, List, Module, PyTrait, Str, str},
    value::Value,
};

/// Identifies an extension function within the registry.
///
/// Packed to fit inline in a `Value` variant alongside the discriminant.
/// The `registry_index` identifies which extension module owns this function,
/// `function_index` identifies which function within that module, and `is_native`
/// determines whether the call is dispatched directly (Rust) or yields to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExtensionFunctionId {
    /// Index into `ExtensionRegistry::entries`.
    pub registry_index: u16,
    /// Index into the extension's function list.
    pub function_index: u16,
    /// The interned name of the function for error messages and host dispatch.
    pub function_name: StringId,
    /// Whether this function executes natively (true) or requires host dispatch (false).
    pub is_native: bool,
}

/// An opaque handle to an extension-managed object, stored on the heap.
///
/// The extension manages the backing storage; Monty only sees the handle ID.
/// Method calls on handles are routed back to the owning extension via
/// `MontyExtension::call_method`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExtensionHandleData {
    /// Index into `ExtensionRegistry::entries` for the owning extension.
    pub registry_index: u16,
    /// Human-readable type name (e.g. `"polars.DataFrame"`).
    pub type_name: String,
    /// Extension-internal identifier for the object.
    pub handle_id: u64,
}

impl HeapItem for ExtensionHandleData {
    fn py_estimate_size(&self) -> usize {
        size_of::<Self>() + self.type_name.len()
    }

    fn py_dec_ref_ids(&mut self, _stack: &mut Vec<HeapId>) {
        // Extension handles don't contain heap references
    }
}

/// Pre-interned string IDs for an extension's function names.
///
/// Created during `intern_names()` (before compilation) so that `create_module()`
/// can use them at runtime without needing mutable access to the intern table.
struct InternedExtension {
    /// Interned module name.
    module_name_id: StringId,
    /// Interned function names, indexed to match `ExtManifest::functions`.
    function_name_ids: Vec<StringId>,
}

/// A single registered extension entry.
///
/// Can be either a native extension (with a trait object for direct dispatch)
/// or a host extension (manifest only, dispatch happens in the host language).
pub(crate) enum ExtensionEntry {
    /// Native extension loaded from a shared library.
    /// Contains the trait object for direct function dispatch.
    Native {
        manifest: ExtManifest,
        extension: MontyExtension_TO<'static, RBox<()>>,
        /// Keep the library handle alive so the `.so`/`.dylib` isn't unloaded.
        _library: Option<libloading::Library>,
    },
    /// Host-backed extension where dispatch happens in the host language.
    /// Only the manifest is stored; the host resolves calls via `CallResult::External`.
    Host { manifest: ExtManifest },
}

impl ExtensionEntry {
    /// Returns the manifest for this extension.
    fn manifest(&self) -> &ExtManifest {
        match self {
            Self::Native { manifest, .. } | Self::Host { manifest } => manifest,
        }
    }
}

/// Registry of loaded extensions, consulted by the compiler and VM during import resolution.
///
/// Extensions are registered before compilation. The compiler checks `lookup()` to decide
/// whether to emit `LoadExtensionModule` or `RaiseImportError`. At runtime, the VM calls
/// `create_module()` to build the module on the heap.
pub struct ExtensionRegistry {
    // Note: Debug is manually implemented because ExtensionEntry contains
    // MontyExtension_TO trait objects that don't derive Debug.
    /// All registered extensions.
    entries: Vec<ExtensionEntry>,
    /// Maps module names to entry indices for O(1) lookup during compilation.
    name_to_index: HashMap<String, u16>,
    /// Pre-interned string IDs, populated by `intern_names()` before compilation.
    interned: Vec<Option<InternedExtension>>,
}

impl fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionRegistry")
            .field("entries_count", &self.entries.len())
            .field("modules", &self.name_to_index.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Default for ExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionRegistry {
    /// Creates an empty extension registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            name_to_index: HashMap::new(),
            interned: Vec::new(),
        }
    }

    /// Returns true if no extensions are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Registers a native extension loaded from a shared library.
    ///
    /// The extension's manifest is read via the trait object, and all functions
    /// are indexed for later lookup. The library handle is kept alive to prevent
    /// the `.so`/`.dylib` from being unloaded.
    ///
    /// # Panics
    ///
    /// Panics if more than `u16::MAX` extensions are registered.
    pub fn register_native(
        &mut self,
        extension: MontyExtension_TO<'static, RBox<()>>,
        library: Option<libloading::Library>,
    ) -> u16 {
        let manifest = extension.manifest();
        let index = u16::try_from(self.entries.len()).expect("too many extensions");
        let module_name = manifest.module_name.to_string();
        self.entries.push(ExtensionEntry::Native {
            manifest,
            extension,
            _library: library,
        });
        self.interned.push(None);
        self.name_to_index.insert(module_name, index);
        index
    }

    /// Registers a host-backed extension from a manifest.
    ///
    /// Only the manifest is stored. When functions from this module are called,
    /// the VM yields `CallResult::External` so the host language can dispatch them.
    ///
    /// # Panics
    ///
    /// Panics if more than `u16::MAX` extensions are registered.
    pub fn register_host(&mut self, manifest: ExtManifest) -> u16 {
        let index = u16::try_from(self.entries.len()).expect("too many extensions");
        let module_name = manifest.module_name.to_string();
        self.entries.push(ExtensionEntry::Host { manifest });
        self.interned.push(None);
        self.name_to_index.insert(module_name, index);
        index
    }

    /// Pre-interns all extension module and function names into the intern table.
    ///
    /// Must be called during the prepare phase (before compilation) while the
    /// `Interns` table is still mutable. After this, `create_module()` can use
    /// the cached `StringId`s without needing `&mut Interns`.
    pub fn intern_names(&mut self, interner: &mut InternerBuilder) {
        for (i, entry) in self.entries.iter().enumerate() {
            let manifest = entry.manifest();
            let module_name_id = interner.intern(manifest.module_name.as_str());
            let function_name_ids = manifest
                .functions
                .iter()
                .map(|f| interner.intern(f.name.as_str()))
                .collect();
            self.interned[i] = Some(InternedExtension {
                module_name_id,
                function_name_ids,
            });
        }
    }

    /// Looks up a module name and returns its registry index if registered.
    ///
    /// Called by the compiler during import resolution. Returns `None` for
    /// unknown modules, which triggers `RaiseImportError` emission.
    #[must_use]
    pub fn lookup(&self, module_name: &str) -> Option<u16> {
        self.name_to_index.get(module_name).copied()
    }

    /// Creates a `Module` on the heap for the extension at `registry_index`.
    ///
    /// Populates the module's attribute dict with `Value::ExtensionFunction` values
    /// for each declared function. The module can then be used like any standard module.
    ///
    /// # Panics
    ///
    /// Panics if `intern_names()` was not called before this method, or if the
    /// registry index is invalid.
    pub(crate) fn create_module(
        &self,
        registry_index: u16,
        vm: &mut VM<'_, '_, impl ResourceTracker>,
    ) -> RunResult<HeapId> {
        let entry = &self.entries[registry_index as usize];
        let manifest = entry.manifest();
        let interned = self.interned[registry_index as usize]
            .as_ref()
            .expect("intern_names() must be called before create_module()");

        let mut module = Module::new(interned.module_name_id);

        for (func_idx, func_decl) in manifest.functions.iter().enumerate() {
            let func_name_id = interned.function_name_ids[func_idx];
            let ext_func = ExtensionFunctionId {
                registry_index,
                function_index: u16::try_from(func_idx).expect("too many functions in extension"),
                function_name: func_name_id,
                is_native: func_decl.is_native,
            };
            module.set_attr(func_name_id, Value::ExtensionFunction(ext_func), vm);
        }

        let heap_id = vm.heap.allocate(HeapData::Module(module))?;
        Ok(heap_id)
    }

    /// Calls a native extension function directly.
    ///
    /// Converts args to `ExtArgs`, calls the extension, and returns the result.
    /// The `budget` is passed to the extension via `ExtContext` so it can
    /// cooperatively check resource limits during long-running operations.
    ///
    /// # Panics
    ///
    /// Panics if the registry index is invalid or the entry is not a native extension.
    pub(crate) fn call_native(
        &self,
        func_id: &ExtensionFunctionId,
        ext_args: ExtArgs,
        interns: &Interns,
        budget: ResourceBudget,
    ) -> ExtResult {
        let entry = &self.entries[func_id.registry_index as usize];
        match entry {
            ExtensionEntry::Native { extension, .. } => {
                let func_name = interns.get_str(func_id.function_name);
                let ctx = ExtContext { budget };
                extension.call(func_name.into(), ext_args, &ctx)
            }
            ExtensionEntry::Host { .. } => {
                panic!("call_native called on host extension — this is a VM bug")
            }
        }
    }

    /// Returns whether the extension at the given index is native (direct Rust dispatch)
    /// as opposed to host-backed (VM suspension).
    pub(crate) fn is_native(&self, registry_index: u16) -> bool {
        matches!(&self.entries[registry_index as usize], ExtensionEntry::Native { .. })
    }

    /// Calls a native extension method on a handle directly.
    ///
    /// Constructs the `ExtHandle` from the `ExtensionHandleData`, then calls the
    /// extension's `call_method` trait method. The `budget` is passed via `ExtContext`
    /// for cooperative resource checking.
    ///
    /// # Panics
    ///
    /// Panics if the registry index is invalid or the entry is not a native extension.
    pub(crate) fn call_method_native(
        &self,
        handle_data: &ExtensionHandleData,
        method_name: &str,
        ext_args: ExtArgs,
        budget: ResourceBudget,
    ) -> ExtResult {
        let entry = &self.entries[handle_data.registry_index as usize];
        match entry {
            ExtensionEntry::Native { extension, .. } => {
                let ext_handle = ExtHandle {
                    type_name: handle_data.type_name.as_str().into(),
                    handle_id: handle_data.handle_id,
                    extension_id: "".into(),
                };
                let ctx = ExtContext { budget };
                extension.call_method(&ext_handle, method_name.into(), ext_args, &ctx)
            }
            ExtensionEntry::Host { .. } => {
                panic!("call_method_native called on host extension — this is a VM bug")
            }
        }
    }

    /// Collects all extension skills for AI agent prompt injection.
    ///
    /// Returns the concatenated skill text from all registered extensions,
    /// separated by `---` dividers.
    #[must_use]
    pub fn extension_skills(&self) -> String {
        self.entries
            .iter()
            .map(|e| e.manifest().skill.as_str())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }

    /// Collects type stub source from all registered extensions.
    ///
    /// Returns the concatenated type stubs as a single string, or `None` if
    /// no extensions provide stubs. Each extension's stubs are separated by
    /// a newline. The result is suitable for passing to the type checker as
    /// a prefix file alongside user-provided type stubs.
    #[must_use]
    pub fn extension_type_stubs(&self) -> Option<String> {
        let stubs: Vec<&str> = self
            .entries
            .iter()
            .filter_map(|e| match &e.manifest().type_stub_source {
                ROption::RSome(s) if !s.is_empty() => Some(s.as_str()),
                _ => None,
            })
            .collect();

        if stubs.is_empty() { None } else { Some(stubs.join("\n")) }
    }
}

// ============================================================================
// Value ↔ ExtValue bridge
// ============================================================================

/// Converts a Monty `Value` to an `ExtValue` for passing to extension functions.
///
/// Only converts types that can cross the ABI boundary. Complex heap types
/// (closures, iterators, etc.) return an error.
pub(crate) fn value_to_ext(value: &Value, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<ExtValue> {
    match value {
        Value::None => Ok(ExtValue::None),
        Value::Bool(b) => Ok(ExtValue::Bool(*b)),
        Value::Int(i) => Ok(ExtValue::Int(*i)),
        Value::Float(f) => Ok(ExtValue::Float(*f)),
        Value::InternString(id) => Ok(ExtValue::Str(vm.interns.get_str(*id).into())),
        Value::InternBytes(id) => Ok(ExtValue::Bytes(vm.interns.get_bytes(*id).to_vec().into())),
        Value::Ref(heap_id) => value_ref_to_ext(*heap_id, vm),
        _ => {
            let ty = value.py_type(vm);
            Err(ExcType::type_error(format!("cannot pass {ty} to extension function")))
        }
    }
}

/// Converts a heap-allocated value to ExtValue.
fn value_ref_to_ext(heap_id: HeapId, vm: &VM<'_, '_, impl ResourceTracker>) -> RunResult<ExtValue> {
    match vm.heap.get(heap_id) {
        HeapData::Str(s) => Ok(ExtValue::Str(s.as_str().into())),
        HeapData::Bytes(b) => Ok(ExtValue::Bytes(b.as_slice().to_vec().into())),
        HeapData::List(list) => {
            let items: Result<Vec<_>, _> = list.as_slice().iter().map(|v| value_to_ext(v, vm)).collect();
            Ok(ExtValue::List(items?.into()))
        }
        HeapData::Dict(dict) => {
            let mut pairs = RVec::new();
            for (key, val) in dict {
                let key_str = match key {
                    Value::InternString(id) => vm.interns.get_str(*id).to_string(),
                    Value::Ref(id) => {
                        if let HeapData::Str(s) = vm.heap.get(*id) {
                            s.as_str().to_string()
                        } else {
                            return Err(ExcType::type_error("dict keys must be strings for extension calls"));
                        }
                    }
                    _ => return Err(ExcType::type_error("dict keys must be strings for extension calls")),
                };
                pairs.push(ExtKeyValue {
                    key: key_str.into(),
                    value: value_to_ext(val, vm)?,
                });
            }
            Ok(ExtValue::Dict(pairs))
        }
        HeapData::ExtensionHandle(handle) => Ok(ExtValue::Handle(ExtHandle {
            type_name: handle.type_name.clone().into(),
            handle_id: handle.handle_id,
            extension_id: "".into(),
        })),
        other => Err(ExcType::type_error(format!(
            "cannot pass {} to extension function",
            other.py_type()
        ))),
    }
}

/// Converts an `ExtValue` from an extension back to a Monty `Value`.
///
/// Primitives become inline values. Strings and bytes are allocated on the heap
/// (since we can't intern during execution). Lists and dicts are allocated on
/// the heap. Handles become `HeapData::ExtensionHandle` entries.
pub(crate) fn ext_to_value(
    ext_value: ExtValue,
    registry_index: u16,
    vm: &mut VM<'_, '_, impl ResourceTracker>,
) -> RunResult<Value> {
    match ext_value {
        ExtValue::None => Ok(Value::None),
        ExtValue::Bool(b) => Ok(Value::Bool(b)),
        ExtValue::Int(i) => Ok(Value::Int(i)),
        ExtValue::Float(f) => Ok(Value::Float(f)),
        ExtValue::Str(s) => {
            let heap_str = Str::from(s.as_str());
            let heap_id = vm.heap.allocate(HeapData::Str(heap_str))?;
            Ok(Value::Ref(heap_id))
        }
        ExtValue::Bytes(b) => {
            let heap_bytes = Bytes::from(b.as_slice());
            let heap_id = vm.heap.allocate(HeapData::Bytes(heap_bytes))?;
            Ok(Value::Ref(heap_id))
        }
        ExtValue::List(items) => {
            let values: Result<Vec<Value>, _> =
                items.into_iter().map(|v| ext_to_value(v, registry_index, vm)).collect();
            let list = List::new(values?);
            let heap_id = vm.heap.allocate(HeapData::List(list))?;
            Ok(Value::Ref(heap_id))
        }
        ExtValue::Dict(pairs) => {
            // Convert each key-value pair: allocate string keys on the heap,
            // recursively convert values, then build a Dict via from_pairs.
            let mut converted_pairs = Vec::with_capacity(pairs.len());
            for kv in pairs {
                let key = str::allocate_string(kv.key.into_string(), vm.heap)?;
                let value = ext_to_value(kv.value, registry_index, vm)?;
                converted_pairs.push((key, value));
            }
            let dict = Dict::from_pairs(converted_pairs, vm)?;
            let heap_id = vm.heap.allocate(HeapData::Dict(dict))?;
            Ok(Value::Ref(heap_id))
        }
        ExtValue::Handle(handle) => {
            let handle_data = ExtensionHandleData {
                registry_index,
                type_name: handle.type_name.to_string(),
                handle_id: handle.handle_id,
            };
            let heap_id = vm.heap.allocate(HeapData::ExtensionHandle(handle_data))?;
            Ok(Value::Ref(heap_id))
        }
    }
}

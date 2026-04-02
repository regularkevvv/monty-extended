//! Stable public API for writing native Monty extensions.
//!
//! This crate defines the ABI-stable boundary that Monty uses to load native
//! extension libraries, plus a higher-level authoring API that hides most of
//! `abi_stable` from extension implementations.
//!
//! # Architecture
//!
//! Native extensions are compiled as `cdylib` crates that export a single
//! `monty_extension_entry()` symbol. Monty loads that symbol at runtime,
//! receives an [`ExtensionEntry`], constructs a [`MontyExtension`] trait object,
//! and dispatches top-level function calls or handle method calls through it.
//!
//! # Authoring Styles
//!
//! ## Module-level (recommended)
//!
//! Apply [`monty_module`] to a `mod` block. Inside it, use:
//!
//! - `#[monty_class]` on structs to declare extension-managed Python types
//! - `#[monty_function()]` on free functions for top-level module exports
//! - `#[monty_methods] impl ClassName` to group handle methods by class
//! - `#[monty_shutdown()]` on a free function for cleanup
//!
//! The macro generates `Extension`, `StoredObject`, typed handles, store/with
//! helpers, the `MontyExtension` trait impl, and the C entry point.
//!
//! ## Impl-level (classic)
//!
//! Use [`monty_classes`] / [`monty_handles`] on an enum and
//! [`monty_module`] / [`monty_extension`] on an impl block. Inside the impl,
//! tag methods with `#[monty_function()]`, `#[monty_method()]`, or
//! `#[monty_shutdown()]` (classic aliases: `#[function()]`, `#[method()]`,
//! `#[shutdown()]`).
//!
//! Both styles produce identical runtime behaviour and ABI output.

use std::{
    collections::{BTreeMap, HashMap},
    hash::BuildHasher,
};

use abi_stable::{
    StableAbi, sabi_trait,
    std_types::{RBox, ROption, RResult, RStr, RString, RVec},
};
pub use monty_extension_macros::{
    monty_class, monty_classes, monty_extension, monty_handles, monty_methods, monty_module,
};

/// Hidden re-exports used by code generated from the proc macros.
///
/// These are public so generated code in downstream crates can name them, but
/// they are intentionally undocumented implementation details.
#[doc(hidden)]
pub mod __private {
    pub use abi_stable::{
        erased_types::TD_Opaque,
        std_types::{RBox, ROption, RResult, RStr, RString, RVec},
    };
}

/// ABI-stable key-value pair for dict entries and keyword arguments.
///
/// Replaces `(RString, ExtValue)` tuples because `abi_stable` requires
/// `#[repr(C)]` structs for ABI stability and arbitrary tuples are not supported.
///
/// Prefer using [`ExtKeyValue::new`] to construct instances so you never need
/// to import `abi_stable` types directly.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtKeyValue {
    /// The key (dict key or keyword argument name).
    pub key: RString,
    /// The value.
    pub value: ExtValue,
}

impl ExtKeyValue {
    /// Creates a new key-value pair from any string-like key and an [`ExtValue`].
    ///
    /// This is the preferred constructor because it hides the `RString` type
    /// from extension authors.
    #[must_use]
    pub fn new(key: impl Into<String>, value: ExtValue) -> Self {
        Self {
            key: RString::from(key.into()),
            value,
        }
    }
}

/// ABI-stable representation of a Python value crossing the extension boundary.
///
/// This mirrors Monty's internal `Value` enum but uses only C-compatible types.
/// Conversion between [`ExtValue`] and Monty's internal representation happens
/// inside the VM bridge, so extension authors never need Monty internals.
///
/// [`ExtValue::Handle`] is the escape hatch for extension-managed objects such as
/// dataframes, ML models, and expression trees that should remain opaque to the VM.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub enum ExtValue {
    /// Python `None`.
    None,
    /// Python `bool`.
    Bool(bool),
    /// Python `int` (fits in `i64`).
    Int(i64),
    /// Python `float`.
    Float(f64),
    /// Python `str`.
    Str(RString),
    /// Python `bytes`.
    Bytes(RVec<u8>),
    /// Python `list`.
    List(RVec<ExtValue>),
    /// Python `dict` with string keys.
    Dict(RVec<ExtKeyValue>),
    /// Opaque handle to an extension-managed object.
    Handle(ExtHandle),
}

impl ExtValue {
    /// Returns the Python-ish type name used in conversion errors.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::None => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::Bytes(_) => "bytes",
            Self::List(_) => "list",
            Self::Dict(_) => "dict",
            Self::Handle(_) => "handle",
        }
    }

    /// Creates a string value from any type that converts to `String`.
    ///
    /// Convenience constructor so extension authors never need to touch
    /// `abi_stable::std_types::RString` directly.
    #[must_use]
    pub fn string(s: impl Into<String>) -> Self {
        Self::Str(RString::from(s.into()))
    }

    /// Creates a list value from any iterator of `ExtValue`.
    ///
    /// Convenience constructor so extension authors never need to touch
    /// `abi_stable::std_types::RVec` directly.
    #[must_use]
    pub fn list(items: impl IntoIterator<Item = ExtValue>) -> Self {
        Self::List(items.into_iter().collect())
    }

    /// Creates a dict value from any iterator of key-value pairs.
    ///
    /// Convenience constructor so extension authors never need to touch
    /// `abi_stable::std_types::RVec` directly.
    #[must_use]
    pub fn dict(pairs: impl IntoIterator<Item = ExtKeyValue>) -> Self {
        Self::Dict(pairs.into_iter().collect())
    }

    /// Returns the contained string slice when this is an `Str` variant.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Returns the contained integer when this is an `Int` variant.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns the contained float when this is a `Float` variant.
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Returns the contained bool when this is a `Bool` variant.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl From<bool> for ExtValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for ExtValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for ExtValue {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<u32> for ExtValue {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<f64> for ExtValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<f32> for ExtValue {
    fn from(value: f32) -> Self {
        Self::Float(f64::from(value))
    }
}

impl From<&str> for ExtValue {
    fn from(value: &str) -> Self {
        Self::Str(RString::from(value))
    }
}

impl From<String> for ExtValue {
    fn from(value: String) -> Self {
        Self::Str(RString::from(value))
    }
}

impl From<Vec<ExtValue>> for ExtValue {
    fn from(value: Vec<ExtValue>) -> Self {
        Self::List(RVec::from(value))
    }
}

impl From<Vec<ExtKeyValue>> for ExtValue {
    fn from(value: Vec<ExtKeyValue>) -> Self {
        Self::Dict(RVec::from(value))
    }
}

/// Opaque handle referencing an extension-managed object.
///
/// The `handle_id` is meaningful only to the extension that created it. Monty
/// stores and passes handles around but never inspects the ID. The `type_name`
/// is used for Python `type()` and error messages, and `extension_id` ties the
/// handle back to the extension that owns it.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtHandle {
    /// Human-readable type name (for example `"polars.DataFrame"`).
    pub type_name: RString,
    /// Extension-internal identifier for the object.
    pub handle_id: u64,
    /// Name of the extension that owns this handle.
    pub extension_id: RString,
}

/// Arguments passed to an extension function call.
///
/// Positional and keyword arguments are kept separate to match Python call
/// semantics. The helper methods on this type provide typed extraction with
/// consistent error messages.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtArgs {
    /// Positional arguments in call order.
    pub positional: RVec<ExtValue>,
    /// Keyword arguments as `(name, value)` pairs.
    pub keyword: RVec<ExtKeyValue>,
}

impl ExtArgs {
    /// Reads a required positional argument and converts it to `T`.
    pub fn get<'a, T>(&'a self, index: usize, function_name: &str, parameter_name: &str) -> ExtValueResult<T>
    where
        T: FromExtValue<'a>,
    {
        let value = self
            .positional
            .get(index)
            .ok_or_else(|| ExtError::missing_argument(function_name, parameter_name))?;
        T::from_ext_value(value, function_name, parameter_name)
    }

    /// Reads a required argument, checking the positional slot first and then the
    /// keyword of the same name.
    pub fn argument<'a, T>(&'a self, index: usize, function_name: &str, parameter_name: &str) -> ExtValueResult<T>
    where
        T: FromExtValue<'a>,
    {
        match self.positional.get(index) {
            Some(value) => T::from_ext_value(value, function_name, parameter_name),
            None => self
                .kwarg(parameter_name, function_name)?
                .ok_or_else(|| ExtError::missing_argument(function_name, parameter_name)),
        }
    }

    /// Reads an optional argument, checking the positional slot first and then a
    /// keyword of the same name.
    pub fn optional_argument<'a, T>(
        &'a self,
        index: usize,
        function_name: &str,
        parameter_name: &str,
    ) -> ExtValueResult<Option<T>>
    where
        T: FromExtValue<'a>,
    {
        match self.positional.get(index) {
            Some(value) => T::from_ext_value(value, function_name, parameter_name).map(Some),
            None => self.kwarg(parameter_name, function_name),
        }
    }

    /// Reads a keyword argument by name and converts it to `T` when present.
    pub fn kwarg<'a, T>(&'a self, name: &str, function_name: &str) -> ExtValueResult<Option<T>>
    where
        T: FromExtValue<'a>,
    {
        self.raw_kwarg(name)
            .map(|value| T::from_ext_value(value, function_name, name))
            .transpose()
    }

    /// Returns the raw keyword value by name when present.
    #[must_use]
    pub fn raw_kwarg(&self, name: &str) -> Option<&ExtValue> {
        self.keyword
            .iter()
            .find(|pair| pair.key.as_str() == name)
            .map(|pair| &pair.value)
    }
}

/// Error returned by extension function calls.
///
/// The `exception_type` should match a Python exception name such as
/// `"ValueError"` or `"TypeError"`. The `message` is the exception payload.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtError {
    /// Python exception type name.
    pub exception_type: RString,
    /// Exception message.
    pub message: RString,
}

impl ExtError {
    /// Creates an error with an explicit Python exception type.
    #[must_use]
    pub fn new(exception_type: impl Into<RString>, message: impl Into<RString>) -> Self {
        Self {
            exception_type: exception_type.into(),
            message: message.into(),
        }
    }

    /// Creates a `TypeError`.
    #[must_use]
    pub fn type_error(message: impl Into<RString>) -> Self {
        Self::new("TypeError", message)
    }

    /// Creates a `ValueError`.
    #[must_use]
    pub fn value_error(message: impl Into<RString>) -> Self {
        Self::new("ValueError", message)
    }

    /// Creates a `KeyError`.
    #[must_use]
    pub fn key_error(message: impl Into<RString>) -> Self {
        Self::new("KeyError", message)
    }

    /// Creates an `AttributeError` for a missing attribute on a Python object.
    #[must_use]
    pub fn attribute_error(type_name: &str, attribute_name: &str) -> Self {
        Self::new(
            "AttributeError",
            format!("'{type_name}' object has no attribute '{attribute_name}'"),
        )
    }

    /// Creates an `AttributeError` for a missing module function.
    #[must_use]
    pub fn unknown_function(module_name: &str, function_name: &str) -> Self {
        Self::new(
            "AttributeError",
            format!("module '{module_name}' has no function '{function_name}'"),
        )
    }

    /// Creates a `TypeError` for a missing required argument.
    #[must_use]
    pub fn missing_argument(function_name: &str, parameter_name: &str) -> Self {
        Self::type_error(format!(
            "{function_name}() missing required argument: '{parameter_name}'"
        ))
    }

    /// Creates a `TypeError` for an argument with the wrong type.
    #[must_use]
    pub fn argument_type(function_name: &str, parameter_name: &str, expected: &str) -> Self {
        Self::type_error(format!(
            "{function_name}() argument '{parameter_name}' must be {expected}"
        ))
    }

    /// Creates a `ValueError` for a dangling or foreign handle ID.
    #[must_use]
    pub fn invalid_handle(function_name: &str, type_name: &str, handle_id: u64) -> Self {
        Self::value_error(format!(
            "{function_name}(): invalid {type_name} handle (id={handle_id})"
        ))
    }

    /// Creates a `ValueError` for integer conversions that exceed Monty's range.
    #[must_use]
    pub fn integer_overflow(type_name: &str) -> Self {
        Self::value_error(format!(
            "{type_name} value does not fit in Monty's 64-bit integer range"
        ))
    }
}

/// Result type for extension helper code before ABI wrapping.
pub type ExtValueResult<T> = Result<T, ExtError>;

/// Result type for extension function calls across the ABI boundary.
pub type ExtResult = RResult<ExtValue, ExtError>;

/// Converts author-friendly return values into ABI-stable [`ExtResult`] values.
///
/// The proc macros call this so extension methods can return `Result<T, ExtError>`
/// with any `T` that implements [`TryIntoExtValue`], or return an [`ExtResult`]
/// directly when they need full manual control.
pub trait IntoExtResult {
    /// Converts `self` into an ABI-stable result.
    fn into_ext_result(self) -> ExtResult;
}

impl IntoExtResult for ExtResult {
    fn into_ext_result(self) -> ExtResult {
        self
    }
}

impl<T> IntoExtResult for Result<T, ExtError>
where
    T: TryIntoExtValue,
{
    fn into_ext_result(self) -> ExtResult {
        match self {
            Ok(value) => match value.try_into_ext_value() {
                Ok(value) => RResult::ROk(value),
                Err(error) => RResult::RErr(error),
            },
            Err(error) => RResult::RErr(error),
        }
    }
}

impl<T> IntoExtResult for T
where
    T: TryIntoExtValue,
{
    fn into_ext_result(self) -> ExtResult {
        match self.try_into_ext_value() {
            Ok(value) => RResult::ROk(value),
            Err(error) => RResult::RErr(error),
        }
    }
}

/// Convenience wrapper around [`IntoExtResult::into_ext_result`].
pub fn into_ext_result(value: impl IntoExtResult) -> ExtResult {
    value.into_ext_result()
}

/// Converts author-friendly Rust values into ABI-stable [`ExtValue`] values.
///
/// This trait intentionally covers the common Rust types extension authors
/// naturally return from business logic so they do not need to construct
/// [`RString`], [`RVec`], or [`ExtValue`] by hand.
pub trait TryIntoExtValue {
    /// Converts `self` into an ABI-safe [`ExtValue`].
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue>;
}

impl TryIntoExtValue for ExtValue {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(self)
    }
}

impl TryIntoExtValue for ExtHandle {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Handle(self))
    }
}

impl TryIntoExtValue for () {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::None)
    }
}

impl TryIntoExtValue for bool {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Bool(self))
    }
}

impl TryIntoExtValue for i64 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Int(self))
    }
}

impl TryIntoExtValue for i32 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Int(i64::from(self)))
    }
}

impl TryIntoExtValue for u32 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Int(i64::from(self)))
    }
}

impl TryIntoExtValue for u64 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        i64::try_from(self)
            .map(ExtValue::Int)
            .map_err(|_| ExtError::integer_overflow("u64"))
    }
}

impl TryIntoExtValue for usize {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        i64::try_from(self)
            .map(ExtValue::Int)
            .map_err(|_| ExtError::integer_overflow("usize"))
    }
}

impl TryIntoExtValue for f64 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Float(self))
    }
}

impl TryIntoExtValue for f32 {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Float(f64::from(self)))
    }
}

impl TryIntoExtValue for String {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Str(self.into()))
    }
}

impl TryIntoExtValue for &str {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Str(self.into()))
    }
}

impl TryIntoExtValue for RString {
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        Ok(ExtValue::Str(self))
    }
}

impl<T> TryIntoExtValue for Option<T>
where
    T: TryIntoExtValue,
{
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        match self {
            Some(value) => value.try_into_ext_value(),
            None => Ok(ExtValue::None),
        }
    }
}

impl<T> TryIntoExtValue for Vec<T>
where
    T: TryIntoExtValue,
{
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        self.into_iter()
            .map(TryIntoExtValue::try_into_ext_value)
            .collect::<ExtValueResult<RVec<_>>>()
            .map(ExtValue::List)
    }
}

impl<T, S> TryIntoExtValue for HashMap<String, T, S>
where
    S: BuildHasher,
    T: TryIntoExtValue,
{
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        self.into_iter()
            .map(|(key, value)| {
                value
                    .try_into_ext_value()
                    .map(|value| ExtKeyValue { key: key.into(), value })
            })
            .collect::<ExtValueResult<RVec<_>>>()
            .map(ExtValue::Dict)
    }
}

impl<T> TryIntoExtValue for BTreeMap<String, T>
where
    T: TryIntoExtValue,
{
    fn try_into_ext_value(self) -> ExtValueResult<ExtValue> {
        self.into_iter()
            .map(|(key, value)| {
                value
                    .try_into_ext_value()
                    .map(|value| ExtKeyValue { key: key.into(), value })
            })
            .collect::<ExtValueResult<RVec<_>>>()
            .map(ExtValue::Dict)
    }
}

/// Extracts typed Rust values from borrowed [`ExtValue`] inputs.
///
/// Implementations may borrow from the input value, which is why the trait is
/// parameterized by the source lifetime.
pub trait FromExtValue<'a>: Sized {
    /// Converts a borrowed [`ExtValue`] into `Self`.
    fn from_ext_value(value: &'a ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self>;
}

impl<'a> FromExtValue<'a> for &'a ExtValue {
    fn from_ext_value(value: &'a ExtValue, _function_name: &str, _parameter_name: &str) -> ExtValueResult<Self> {
        Ok(value)
    }
}

impl FromExtValue<'_> for ExtValue {
    fn from_ext_value(value: &ExtValue, _function_name: &str, _parameter_name: &str) -> ExtValueResult<Self> {
        Ok(value.clone())
    }
}

impl FromExtValue<'_> for bool {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Bool(value) => Ok(*value),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a bool")),
        }
    }
}

impl FromExtValue<'_> for i64 {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Int(value) => Ok(*value),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "an int")),
        }
    }
}

impl FromExtValue<'_> for usize {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Int(value) => usize::try_from(*value)
                .map_err(|_| ExtError::argument_type(function_name, parameter_name, "a non-negative int")),
            _ => Err(ExtError::argument_type(
                function_name,
                parameter_name,
                "a non-negative int",
            )),
        }
    }
}

impl FromExtValue<'_> for f64 {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Float(value) => Ok(*value),
            ExtValue::Int(value) => Ok(*value as f64),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a number")),
        }
    }
}

impl<'a> FromExtValue<'a> for &'a str {
    fn from_ext_value(value: &'a ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Str(value) => Ok(value.as_str()),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a str")),
        }
    }
}

impl FromExtValue<'_> for String {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Str(value) => Ok(value.to_string()),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a str")),
        }
    }
}

impl<'a> FromExtValue<'a> for &'a [u8] {
    fn from_ext_value(value: &'a ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Bytes(value) => Ok(value.as_slice()),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "bytes")),
        }
    }
}

impl<'a> FromExtValue<'a> for &'a ExtHandle {
    fn from_ext_value(value: &'a ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Handle(handle) => Ok(handle),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a handle")),
        }
    }
}

impl FromExtValue<'_> for ExtHandle {
    fn from_ext_value(value: &ExtValue, function_name: &str, parameter_name: &str) -> ExtValueResult<Self> {
        match value {
            ExtValue::Handle(handle) => Ok(handle.clone()),
            _ => Err(ExtError::argument_type(function_name, parameter_name, "a handle")),
        }
    }
}

/// Marker trait implemented by macro-generated typed handles.
///
/// The proc macros use this to recover the exported Python type name from a
/// handle parameter and route method calls without string duplication in the
/// extension implementation.
pub trait MontyHandleType: Clone + TryIntoExtValue + for<'a> FromExtValue<'a> {
    /// Fully-qualified exported handle type name such as `"polars.DataFrame"`.
    const TYPE_NAME: &'static str;

    /// Returns the borrowed ABI handle backing this typed handle.
    fn as_ext_handle(&self) -> &ExtHandle;

    /// Consumes the typed wrapper and returns the ABI handle.
    fn into_ext_handle(self) -> ExtHandle;
}

/// Resource budget exposed to extensions for cooperative limit checking.
///
/// Native extensions should check these limits during long-running operations
/// when they can do so cheaply. The VM also enforces limits around calls, but
/// cooperative checking enables earlier termination and better error locality.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ResourceBudget {
    /// Remaining wall-clock time in milliseconds, or `RNone` for unlimited.
    pub remaining_time_ms: ROption<u64>,
    /// Remaining allocation count, or `RNone` for unlimited.
    pub remaining_allocations: ROption<u64>,
}

/// Execution context passed to extension function calls.
///
/// Today this carries only resource-budget information, but it intentionally
/// exists as a dedicated type so the API can grow without changing every
/// function signature.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtContext {
    /// Current resource budget.
    pub budget: ResourceBudget,
}

/// Declaration of a single function exported by an extension.
///
/// Used in [`ExtManifest::functions`] so the VM knows which top-level names
/// exist and whether they dispatch natively or through the host language.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtFunctionDecl {
    /// Function name as it appears in Python.
    pub name: RString,
    /// Whether this function executes natively in the VM loop.
    pub is_native: bool,
}

/// Module metadata describing an extension's capabilities.
///
/// Every extension, native or host-backed, produces a manifest. Monty uses it
/// to register imports, populate module attributes, inject type stubs, and
/// collect skill text for agent prompts.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct ExtManifest {
    /// Python module name.
    pub module_name: RString,
    /// Functions exported by this module.
    pub functions: RVec<ExtFunctionDecl>,
    /// Optional Python type stub source.
    pub type_stub_source: ROption<RString>,
    /// Markdown skill description for AI agent prompt injection.
    pub skill: RString,
    /// Semantic version of this extension.
    pub version: RString,
}

/// The core trait that every native extension implements.
///
/// Most new extensions should use [`monty_module`] (or the older
/// [`monty_extension`] alias) instead of implementing this trait by hand. The
/// trait remains public because it is the actual ABI contract consumed by the
/// runtime.
#[sabi_trait]
pub trait MontyExtension: Send + Sync {
    /// Returns the extension manifest describing its module and functions.
    fn manifest(&self) -> ExtManifest;

    /// Calls a top-level function exported by this extension.
    fn call(&self, function_name: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;

    /// Calls a method on a handle owned by this extension.
    fn call_method(&self, handle: &ExtHandle, method: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;

    /// Called when the extension is being unloaded.
    fn shutdown(&self);
}

/// Current extension API version used for compatibility checks.
pub const API_VERSION: u32 = 1;

/// C-ABI entry point exported by native extension libraries.
///
/// Monty looks up `monty_extension_entry`, checks [`API_VERSION`], and calls
/// `create` to obtain the trait object used for all dispatch.
#[repr(C)]
#[derive(StableAbi)]
pub struct ExtensionEntry {
    /// API version this extension was compiled against.
    pub api_version: u32,
    /// Factory function that creates the extension trait object.
    pub create: extern "C" fn() -> MontyExtension_TO<'static, RBox<()>>,
}

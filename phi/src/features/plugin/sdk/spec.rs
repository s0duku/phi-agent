// The Python SDK is the stable contract exposed to plugin authors.
// Backends may change from subprocess to embedded runtimes later, but the
// public `import phi` surface should evolve from this spec layer instead of
// being redefined ad-hoc inside a specific worker implementation.

pub(crate) const PHI_PYTHON_MODULE_NAME: &str = "phi";
pub(crate) const PHI_PYTHON_SDK_VERSION: &str = "0.1.0";

// Capabilities are intentionally narrow today. They advertise what the runtime
// actually exposes to Python plugins so later expansions can happen without
// changing the meaning of older names.
pub(crate) const PHI_PYTHON_SDK_CAPABILITIES: &[&str] =
    &["sdk_version", "capabilities", "tool_registry"];

mod spec;

pub(crate) use spec::{
    PHI_PYTHON_MODULE_NAME, PHI_PYTHON_SDK_CAPABILITIES, PHI_PYTHON_SDK_VERSION,
};

pub(crate) fn python_module_source() -> &'static str {
    crate::features::plugin::python::sdk_module_source()
}

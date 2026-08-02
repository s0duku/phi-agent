# Serialization

Phi's internal JSON is a canonical representation of the current Rust types.
Session input is UTF-8 JSON, and unknown fields or historical aliases are not part
of the current format.

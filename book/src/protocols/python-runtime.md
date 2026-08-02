# Python runtime protocol

The Python worker protocol is a private, evolving wire format. Requests and
responses use explicit tagged variants and reject unknown fields.

The protocol is not a public compatibility contract.

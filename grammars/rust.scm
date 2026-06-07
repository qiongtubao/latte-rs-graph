; Rust grammar queries for code graph extraction

; === Definitions ===

(function_item
  name: (identifier) @definition
  (#set! kind "function"))

(struct_item
  name: (type_identifier) @definition
  (#set! kind "struct"))

(enum_item
  name: (type_identifier) @definition
  (#set! kind "enum"))

(trait_item
  name: (type_identifier) @definition
  (#set! kind "trait"))

(type_item
  name: (type_identifier) @definition
  (#set! kind "type_alias"))

(const_item
  name: (identifier) @definition
  (#set! kind "constant"))

(static_item
  name: (identifier) @definition
  (#set! kind "variable"))

; === Calls ===

(call_expression
  function: (identifier) @call
  (#set! call-name "direct"))

(call_expression
  function: (field_expression
    field: (field_identifier) @call)
  (#set! call-name "method"))

; === Imports ===

(use_declaration
  argument: (identifier) @import
  (#set! import-target "direct"))

; === Heritage ===

(impl_item
  trait: (_) @heritage
  (#set! heritage-kind "implements"))
(impl_item
  type: (_) @heritage
  (#set! heritage-kind "implements"))

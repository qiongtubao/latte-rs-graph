; C grammar queries for code graph extraction

; === Definitions ===

(function_definition
  declarator: (function_declarator
    declarator: (identifier) @definition)
  (#set! kind "function"))

(struct_specifier
  name: (type_identifier) @definition
  (#set! kind "struct"))

(enum_specifier
  name: (type_identifier) @definition
  (#set! kind "enum"))

(union_specifier
  name: (type_identifier) @definition
  (#set! kind "type_alias"))

; Type definitions
(type_definition
  name: (type_identifier) @definition
  (#set! kind "type_alias"))

; === Calls ===

(call_expression
  function: (identifier) @call
  (#set! call-name "direct"))

(call_expression
  function: (field_expression
    field: (field_identifier) @call)
  (#set! call-name "method"))

; === Imports ===

(preproc_include
  path: (string_literal) @import
  (#set! import-target "header"))

(preproc_include
  path: (system_lib_string) @import
  (#set! import-target "system"))

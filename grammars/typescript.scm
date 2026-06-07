; TypeScript/TSX grammar queries for code graph extraction

; === Definitions ===

(function_declaration
  name: (identifier) @definition
  (#set! kind "function"))

(method_definition
  name: (property_identifier) @definition
  (#set! kind "method"))

(variable_declarator
  name: (identifier) @definition
  value: (arrow_function)
  (#set! kind "function"))

(class_declaration
  name: (type_identifier) @definition
  (#set! kind "class"))

(interface_declaration
  name: (type_identifier) @definition
  (#set! kind "interface"))

(type_alias_declaration
  name: (type_identifier) @definition
  (#set! kind "type_alias"))

(enum_declaration
  name: (identifier) @definition
  (#set! kind "enum"))

; === Calls ===

(call_expression
  function: (identifier) @call
  (#set! call-name "direct"))

(call_expression
  function: (member_expression
    property: (property_identifier) @call)
  (#set! call-name "method"))

; === Imports ===

(import_statement
  source: (string (string_fragment) @import)
  (#set! import-target "module"))

; === Heritage ===

(class_declaration
  heritage: (class_heritage
    (_) @heritage
    (#set! heritage-kind "extends")))

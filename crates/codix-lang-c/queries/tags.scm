; Written for codix after tree-sitter-c 0.24.2, queries/tags.scm, which
; captures a function's declarator rather than the whole definition; here
; the definition is the whole function, struct, enum or typedef.

(function_definition
  declarator: [
    (function_declarator
      declarator: (identifier) @name)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @name))
    (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator
          declarator: (identifier) @name)))
  ]) @definition.function

; A prototype: the function's name, without its body.
(declaration
  declarator: [
    (function_declarator
      declarator: (identifier) @name)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @name))
  ]) @definition.declaration

(struct_specifier
  name: (type_identifier) @name
  body: (_)) @definition.struct

(union_specifier
  name: (type_identifier) @name
  body: (_)) @definition.struct

(enum_specifier
  name: (type_identifier) @name
  body: (_)) @definition.enum

(enumerator
  name: (identifier) @name) @definition.enum_member

(field_declaration
  declarator: [
    (field_identifier) @name
    (pointer_declarator
      declarator: (field_identifier) @name)
    (array_declarator
      declarator: (field_identifier) @name)
  ]) @definition.field

(type_definition
  declarator: [
    (type_identifier) @name
    (pointer_declarator
      declarator: (type_identifier) @name)
  ]) @definition.type_alias

(preproc_def
  name: (identifier) @name) @definition.macro

(preproc_function_def
  name: (identifier) @name) @definition.macro

(translation_unit
  (declaration
    declarator: [
      (identifier) @name
      (init_declarator
        declarator: (identifier) @name)
      (init_declarator
        declarator: (pointer_declarator
          declarator: (identifier) @name))
      (init_declarator
        declarator: (array_declarator
          declarator: (identifier) @name))
      (pointer_declarator
        declarator: (identifier) @name)
      (array_declarator
        declarator: (identifier) @name)
      (pointer_declarator
        declarator: (array_declarator
          declarator: (identifier) @name))
    ]) @definition.variable)

(call_expression
  function: (identifier) @name) @reference.call

; Added for codix: includes, as written.

(preproc_include path: (string_literal) @include)

((preproc_include path: (system_lib_string) @include)
  (#set! include.system))

; Adapted from tree-sitter-go 0.25.0, queries/tags.scm: types are split by
; what they define, a method's receiver type is its parent, and the
; references are kept as upstream has them. Doc comment captures are dropped.

(function_declaration
  name: (identifier) @name) @definition.function

(method_declaration
  receiver: (parameter_list
    (parameter_declaration
      type: [
        (type_identifier) @parent
        (pointer_type (type_identifier) @parent)
        (generic_type type: (type_identifier) @parent)
        (pointer_type (generic_type type: (type_identifier) @parent))
      ]))
  name: (field_identifier) @name) @definition.method

(call_expression
  function: [
    (identifier) @name
    (parenthesized_expression (identifier) @name)
    (selector_expression field: (field_identifier) @name)
    (parenthesized_expression (selector_expression field: (field_identifier) @name))
  ]) @reference.call

(type_identifier) @name @reference.type

; Added for codix.

(type_spec
  name: (type_identifier) @name
  type: (struct_type)) @definition.struct

(type_spec
  name: (type_identifier) @name
  type: (interface_type)) @definition.interface

(type_spec
  name: (type_identifier) @name
  type: [
    (type_identifier)
    (qualified_type)
    (pointer_type)
    (function_type)
    (map_type)
    (slice_type)
    (array_type)
    (channel_type)
    (generic_type)
  ]) @definition.type_alias

(type_alias
  name: (type_identifier) @name) @definition.type_alias

(field_declaration
  name: (field_identifier) @name) @definition.field

(method_elem
  name: (field_identifier) @name) @definition.method

;; The `name` field also holds the commas between names, so names are
;; matched as direct identifier children (values and types are nested or of
;; other kinds).
(source_file
  (const_declaration
    (const_spec
      (identifier) @name) @definition.constant))

(source_file
  (var_declaration
    (var_spec
      (identifier) @name) @definition.variable))

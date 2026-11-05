; Written for codix: tree-sitter-kotlin-ng 1.1.0 ships no tags query.
; Later patterns win over earlier ones for the same node.

(class_declaration
  name: (identifier) @name) @definition.class

(class_declaration
  "interface"
  name: (identifier) @name) @definition.interface

(class_declaration
  name: (identifier) @name
  (enum_class_body)) @definition.enum

(object_declaration
  name: (identifier) @name) @definition.class

(companion_object
  name: (identifier) @name) @definition.class

(enum_entry
  (identifier) @name) @definition.enum_member

(function_declaration
  name: (identifier) @name) @definition.function

(class_body
  (function_declaration
    name: (identifier) @name) @definition.method)

(enum_class_body
  (function_declaration
    name: (identifier) @name) @definition.method)

(secondary_constructor
  "constructor" @name) @definition.constructor

(class_parameter
  ["val" "var"]
  (identifier) @name) @definition.property

(property_declaration
  (variable_declaration
    (identifier) @name)) @definition.property

(source_file
  (property_declaration
    (variable_declaration
      (identifier) @name)) @definition.variable)

(source_file
  (property_declaration
    (modifiers
      (property_modifier) @const
      (#eq? @const "const"))
    (variable_declaration
      (identifier) @name)) @definition.constant)

(type_alias
  type: (identifier) @name) @definition.type_alias

(call_expression
  (identifier) @name) @reference.call

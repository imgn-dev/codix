; From tree-sitter-java 0.23.5, queries/tags.scm.

(class_declaration
  name: (identifier) @name) @definition.class

(method_declaration
  name: (identifier) @name) @definition.method

(method_invocation
  name: (identifier) @name
  arguments: (argument_list) @reference.call)

(interface_declaration
  name: (identifier) @name) @definition.interface

(type_list
  (type_identifier) @name) @reference.implementation

(object_creation_expression
  type: (type_identifier) @name) @reference.class

(superclass (type_identifier) @name) @reference.class

; Added for codix: the definitions upstream leaves out.

(enum_declaration
  name: (identifier) @name) @definition.enum

(enum_constant
  name: (identifier) @name) @definition.enum_member

(record_declaration
  name: (identifier) @name) @definition.record

(annotation_type_declaration
  name: (identifier) @name) @definition.interface

(annotation_type_element_declaration
  name: (identifier) @name) @definition.method

(constructor_declaration
  name: (identifier) @name) @definition.constructor

(compact_constructor_declaration
  name: (identifier) @name) @definition.constructor

(field_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @definition.field

(constant_declaration
  declarator: (variable_declarator
    name: (identifier) @name)) @definition.constant

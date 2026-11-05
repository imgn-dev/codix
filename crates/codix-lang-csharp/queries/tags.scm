; From tree-sitter-c-sharp 0.23.5, queries/tags.scm, without its bare `@module`
; capture.

(class_declaration name: (identifier) @name) @definition.class
(class_declaration (base_list (_) @name)) @reference.class
(interface_declaration name: (identifier) @name) @definition.interface
(interface_declaration (base_list (_) @name)) @reference.interface
(method_declaration name: (identifier) @name) @definition.method
(object_creation_expression type: (identifier) @name) @reference.class
(type_parameter_constraints_clause (identifier) @name) @reference.class
(type_parameter_constraint (type type: (identifier) @name)) @reference.class
(variable_declaration type: (identifier) @name) @reference.class
(invocation_expression function: (member_access_expression name: (identifier) @name)) @reference.send
(namespace_declaration name: (identifier) @name) @definition.module

; Added for codix. Later patterns win over the ones above for the same node.

(namespace_declaration
  name: [(identifier) (qualified_name)] @name) @definition.namespace

(file_scoped_namespace_declaration
  name: [(identifier) (qualified_name)] @name) @definition.namespace

(struct_declaration
  name: (identifier) @name) @definition.struct

(record_declaration
  name: (identifier) @name) @definition.class

(enum_declaration
  name: (identifier) @name) @definition.enum

(enum_member_declaration
  name: (identifier) @name) @definition.enum_member

(delegate_declaration
  name: (identifier) @name) @definition.type_alias

(constructor_declaration
  name: (identifier) @name) @definition.constructor

(destructor_declaration
  name: (identifier) @name) @definition.method

(property_declaration
  name: (identifier) @name) @definition.property

(field_declaration
  (variable_declaration
    (variable_declarator
      name: (identifier) @name))) @definition.field

(field_declaration
  (modifier) @const
  (variable_declaration
    (variable_declarator
      name: (identifier) @name))
  (#eq? @const "const")) @definition.constant

(event_field_declaration
  (variable_declaration
    (variable_declarator
      name: (identifier) @name))) @definition.field

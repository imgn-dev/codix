; Written for codix: scopes, local definitions and references of Rust.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; Items are visible in their whole block; `let` bindings only after their
; statement. A module does not see the names around it.
(function_item) @local.scope.function
(closure_expression) @local.scope.function
(block) @local.scope.block
(for_expression) @local.scope.block
(while_expression) @local.scope.block
(if_expression) @local.scope.block
(match_arm) @local.scope.block
(mod_item body: (declaration_list) @local.scope.module)
(impl_item body: (declaration_list) @local.scope.class)
(trait_item body: (declaration_list) @local.scope.class)

; Generic parameters are visible in the whole item.
(struct_item) @local.scope.type
(enum_item) @local.scope.type
(trait_item) @local.scope.type
(impl_item) @local.scope.type
(type_item) @local.scope.type

; Definitions
;------------

; A lowercase name in a pattern binds; a capitalized one names a constant,
; a unit struct or an enum variant.
((let_declaration pattern: (identifier) @local.definition.variable) @local.after
  (#match? @local.definition.variable "^[a-z_]"))
((let_declaration
  pattern: (_ (identifier) @local.definition.variable)) @local.after
  (#match? @local.definition.variable "^[a-z_]"))
((let_condition pattern: (_ (identifier) @local.definition.variable))
  (#match? @local.definition.variable "^[a-z_]"))
((let_condition pattern: (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
((parameter pattern: (identifier) @local.definition.parameter)
  (#match? @local.definition.parameter "^[a-z_]"))
((parameter pattern: (_ (identifier) @local.definition.parameter))
  (#match? @local.definition.parameter "^[a-z_]"))
((closure_parameters (identifier) @local.definition.parameter)
  (#match? @local.definition.parameter "^[a-z_]"))
((for_expression pattern: (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
((for_expression pattern: (_ (identifier) @local.definition.variable))
  (#match? @local.definition.variable "^[a-z_]"))
; The pattern of a match arm is its first child; a guard follows it.
((match_pattern . (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
((match_pattern . (_ (identifier) @local.definition.variable))
  (#match? @local.definition.variable "^[a-z_]"))
((tuple_struct_pattern (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
((tuple_pattern (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
((captured_pattern (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))
(field_pattern name: (shorthand_field_identifier) @local.definition.variable)
((field_pattern pattern: (identifier) @local.definition.variable)
  (#match? @local.definition.variable "^[a-z_]"))

(type_parameters (type_parameter name: (type_identifier) @local.definition.type_parameter))

(use_declaration argument: (identifier) @local.definition.import)
(use_declaration argument: (scoped_identifier name: (identifier) @local.definition.import))
(use_as_clause alias: (identifier) @local.definition.import)
(use_list (identifier) @local.definition.import)
(use_list (scoped_identifier name: (identifier) @local.definition.import))

; References
;-----------

(identifier) @local.reference.read
(type_identifier) @local.reference.type
(shorthand_field_initializer (identifier) @local.reference.read)

(call_expression function: (identifier) @local.reference.call)
(assignment_expression left: (identifier) @local.reference.write)
(compound_assignment_expr left: (identifier) @local.reference.write)

(field_expression
  value: (_) @local.receiver
  field: (field_identifier) @local.reference.read)
(call_expression
  function: (field_expression
    value: (_) @local.receiver
    field: (field_identifier) @local.reference.call))
(assignment_expression
  left: (field_expression
    value: (_) @local.receiver
    field: (field_identifier) @local.reference.write))
(compound_assignment_expr
  left: (field_expression
    value: (_) @local.receiver
    field: (field_identifier) @local.reference.write))

(scoped_identifier
  path: (_) @local.receiver
  name: (identifier) @local.reference.read)
(call_expression
  function: (scoped_identifier
    path: (_) @local.receiver
    name: (identifier) @local.reference.call))
(scoped_type_identifier
  path: (_) @local.receiver
  name: (type_identifier) @local.reference.type)

; Names that are no references.
(macro_invocation macro: (identifier) @local.skip)
(lifetime (identifier) @local.skip)
(label (identifier) @local.skip)
(use_declaration argument: (scoped_identifier path: (identifier) @local.skip))

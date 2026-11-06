; Written for codix: scopes, local definitions and references of PHP.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; A variable belongs to its whole function: the first assignment defines
; it, later ones write it. A function does not see the variables around it,
; except those it names in `global` or, for a closure, in `use`; an arrow
; function sees them all.
(function_definition) @local.scope.function
(method_declaration) @local.scope.function
(anonymous_function) @local.scope.function
(arrow_function) @local.scope.arrow
; Members are reached through `$this`, `self` or `static` only, so
; inherited ones never hide a plain name.
(declaration_list) @local.scope.class

; Definitions
;------------

(simple_parameter name: (variable_name (name) @local.definition.parameter))
(variadic_parameter name: (variable_name (name) @local.definition.parameter))
(property_promotion_parameter name: (variable_name (name) @local.definition.parameter))
(catch_clause name: (variable_name (name) @local.definition.variable))
(static_variable_declaration name: (variable_name (name) @local.definition.variable))

((assignment_expression left: (variable_name (name) @local.definition.variable))
  (#set! local.reuse))
((assignment_expression left: (list_literal (variable_name (name) @local.definition.variable)))
  (#set! local.reuse))
((foreach_statement (variable_name) (variable_name (name) @local.definition.variable))
  (#set! local.reuse))
((foreach_statement (pair (variable_name (name) @local.definition.variable)))
  (#set! local.reuse))

(global_declaration (variable_name (name) @local.global))
(anonymous_function_use_clause (variable_name (name) @local.nonlocal))

(namespace_use_clause alias: (name) @local.definition.import)
(namespace_use_clause !alias (name) @local.definition.import)
(namespace_use_clause !alias (qualified_name (name) @local.definition.import))

; References
;-----------

(name) @local.reference.read
(variable_name (name) @local.reference.read)
(named_type (name) @local.reference.type)
(object_creation_expression (name) @local.reference.type)

(function_call_expression function: (name) @local.reference.call)
(assignment_expression left: (variable_name (name) @local.reference.write))
(augmented_assignment_expression left: (variable_name (name) @local.reference.write))
(update_expression argument: (variable_name (name) @local.reference.write))

(member_access_expression
  object: (_) @local.receiver
  name: (name) @local.reference.read)
(member_call_expression
  object: (_) @local.receiver
  name: (name) @local.reference.call)
(nullsafe_member_access_expression
  object: (_) @local.receiver
  name: (name) @local.reference.read)
(nullsafe_member_call_expression
  object: (_) @local.receiver
  name: (name) @local.reference.call)
(scoped_call_expression
  scope: (_) @local.receiver
  name: (name) @local.reference.call)
(class_constant_access_expression
  (_) @local.receiver
  .
  (name) @local.reference.read
  .)
(assignment_expression
  left: (member_access_expression
    object: (_) @local.receiver
    name: (name) @local.reference.write))

; Names that are no references.
(namespace_name (name) @local.skip)
((variable_name (name) @local.skip)
  (#eq? @local.skip "this"))
(argument name: (name) @local.skip)

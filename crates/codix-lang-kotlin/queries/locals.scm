; Written for codix: scopes, local definitions and references of Kotlin.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; Members are visible in the whole body. A class with supertypes, an
; object expression or an enum may inherit members codix cannot see.
(class_body) @local.scope.class
(enum_class_body) @local.scope.class
(class_declaration
  (delegation_specifiers)
  (class_body) @local.scope.class
  (#set! local.opaque))
((enum_class_body) @local.scope.class
  (#set! local.opaque))
(object_declaration
  (delegation_specifiers)
  (class_body) @local.scope.class
  (#set! local.opaque))
(companion_object
  (delegation_specifiers)
  (class_body) @local.scope.class
  (#set! local.opaque))
(object_literal
  (class_body) @local.scope.class
  (#set! local.opaque))

; Type parameters and constructor parameters are visible in the whole class.
(class_declaration) @local.scope.type

(function_declaration) @local.scope.function
(secondary_constructor) @local.scope.function
(anonymous_function) @local.scope.function
(getter) @local.scope.function
(setter) @local.scope.function

; A lambda may have a receiver whose members hide the file's symbols,
; though not locals; which lambdas have one needs types.
((lambda_literal) @local.scope.function
  (#set! local.opaque "members"))

(block) @local.scope.block
(for_statement) @local.scope.block
(catch_block) @local.scope.block
(when_entry) @local.scope.block

; Definitions
;------------

(parameter (identifier) @local.definition.parameter)
(class_parameter (identifier) @local.definition.parameter)
(lambda_parameters (variable_declaration (identifier) @local.definition.parameter))
(lambda_parameters
  (multi_variable_declaration (variable_declaration (identifier) @local.definition.parameter)))
(for_statement (variable_declaration (identifier) @local.definition.variable))
(for_statement
  (multi_variable_declaration (variable_declaration (identifier) @local.definition.variable)))
(property_declaration
  (multi_variable_declaration (variable_declaration (identifier) @local.definition.variable)))
(catch_block (identifier) @local.definition.parameter)
(type_parameter (identifier) @local.definition.type_parameter)

(import (qualified_identifier (identifier) @local.definition.import .) .)
(import (identifier) @local.definition.import)

; References
;-----------

(identifier) @local.reference.read
(user_type (identifier) @local.reference.type)

(call_expression . (identifier) @local.reference.call)
(assignment left: (identifier) @local.reference.write)
(unary_expression
  operator: ["++" "--"]
  argument: (identifier) @local.reference.write)

(navigation_expression
  (_) @local.receiver
  (identifier) @local.reference.read
  .)
(call_expression
  .
  (navigation_expression
    (_) @local.receiver
    (identifier) @local.reference.call
    .))
(assignment
  left: (navigation_expression
    (_) @local.receiver
    (identifier) @local.reference.write
    .))
(infix_expression
  (_) @local.receiver
  .
  (identifier) @local.reference.call)

; A named argument names a parameter of the callee.
(call_expression
  (_) @local.receiver
  (value_arguments
    (value_argument
      (identifier) @local.reference.read
      "=")))

; Names that are no references.
(package_header (qualified_identifier (identifier) @local.skip))
(import (qualified_identifier (identifier) @local.skip))

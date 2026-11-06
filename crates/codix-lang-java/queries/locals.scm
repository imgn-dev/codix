; Written for codix: scopes, local definitions and references of Java.
; General patterns come first: for a node captured as a reference by
; several patterns, the one written last wins.

; Scopes
;-------

; Class bodies: members are visible in the whole body. A class with a
; superclass or interfaces may inherit members codix cannot see, and so may
; enums, anonymous classes and enum constants with a body.
(class_body) @local.scope.class
(interface_body) @local.scope.class
(enum_body) @local.scope.class

(class_declaration
  superclass: (_)
  body: (class_body) @local.scope.class
  (#set! local.opaque))
(class_declaration
  interfaces: (_)
  body: (class_body) @local.scope.class
  (#set! local.opaque))
(record_declaration
  interfaces: (_)
  body: (class_body) @local.scope.class
  (#set! local.opaque))
(interface_declaration
  (extends_interfaces)
  body: (interface_body) @local.scope.class
  (#set! local.opaque))
((enum_body) @local.scope.class
  (#set! local.opaque))
(object_creation_expression
  (class_body) @local.scope.class
  (#set! local.opaque))
(enum_constant
  body: (class_body) @local.scope.class
  (#set! local.opaque))

; Type parameters and record components are visible in the header and body.
(class_declaration) @local.scope.type
(interface_declaration) @local.scope.type
(record_declaration) @local.scope.type

(method_declaration) @local.scope.function
(constructor_declaration) @local.scope.function
(compact_constructor_declaration) @local.scope.function
(lambda_expression) @local.scope.function

(block) @local.scope.block
(constructor_body) @local.scope.block
(for_statement) @local.scope.block
(enhanced_for_statement) @local.scope.block
(catch_clause) @local.scope.block
(try_with_resources_statement) @local.scope.block
(switch_block_statement_group) @local.scope.block
(switch_rule) @local.scope.block

; A constant in a case label names an enum constant of the switched type
; or a constant in scope; which one needs types.
((switch_label) @local.scope.block
  (#set! local.opaque))

; Definitions
;------------

(formal_parameter name: (identifier) @local.definition.parameter)
(spread_parameter (variable_declarator name: (identifier) @local.definition.parameter))
(catch_formal_parameter name: (identifier) @local.definition.parameter)
(inferred_parameters (identifier) @local.definition.parameter)
(lambda_expression parameters: (identifier) @local.definition.parameter)

(local_variable_declaration
  declarator: (variable_declarator name: (identifier) @local.definition.variable))
(enhanced_for_statement name: (identifier) @local.definition.variable)
(resource name: (identifier) @local.definition.variable)
(instanceof_expression name: (identifier) @local.definition.variable)
(type_pattern (identifier) @local.definition.variable)
(record_pattern_component (identifier) @local.definition.variable)

(record_declaration
  parameters: (formal_parameters
    (formal_parameter name: (identifier) @local.definition.field)))

(type_parameter (type_identifier) @local.definition.type_parameter)

; References
;-----------

(identifier) @local.reference.read
(type_identifier) @local.reference.type

(assignment_expression left: (identifier) @local.reference.write)
(update_expression (identifier) @local.reference.write)

(method_invocation
  !object
  name: (identifier) @local.reference.call)
(method_invocation
  object: (_) @local.receiver
  name: (identifier) @local.reference.call)
(method_reference
  .
  (_) @local.receiver
  (identifier) @local.reference.call
  .)

(field_access
  object: (_) @local.receiver
  field: (identifier) @local.reference.read)
(assignment_expression
  left: (field_access
    object: (_) @local.receiver
    field: (identifier) @local.reference.write))

(scoped_type_identifier
  (_) @local.receiver
  .
  (type_identifier) @local.reference.type
  .)
(scoped_identifier
  scope: (_) @local.receiver
  name: (identifier) @local.reference.read)

(import_declaration (identifier) @local.reference.import)
(import_declaration
  (scoped_identifier
    scope: (_) @local.receiver
    name: (identifier) @local.reference.import))

(marker_annotation name: (identifier) @local.reference.type)
(annotation name: (identifier) @local.reference.type)
(annotation
  name: (_) @local.receiver
  arguments: (annotation_argument_list
    (element_value_pair key: (identifier) @local.reference.call)))

; Names that are no references.
(package_declaration (identifier) @local.skip)
(package_declaration (scoped_identifier name: (identifier) @local.skip))
(package_declaration (scoped_identifier scope: (identifier) @local.skip))
(labeled_statement (identifier) @local.skip)
(break_statement (identifier) @local.skip)
(continue_statement (identifier) @local.skip)

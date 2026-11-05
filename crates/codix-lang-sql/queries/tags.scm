; Written for codix: the objects that statements create.

(create_table
  (object_reference
    name: (identifier) @name)) @definition.struct

(create_view
  (object_reference
    name: (identifier) @name)) @definition.struct

(create_function
  (object_reference
    name: (identifier) @name)) @definition.function

(create_schema
  (identifier) @name) @definition.namespace

(create_type
  (object_reference
    name: (identifier) @name)) @definition.type_alias

(create_sequence
  (object_reference
    name: (identifier) @name)) @definition.variable

(column_definition
  name: (identifier) @name) @definition.field

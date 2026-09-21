"""
OberihStruct — справжній struct тип для рантайму Oberih.

Замість dict {"__type__": "Point", "x": 3, "y": 4}:
  - OberihStruct("Point", {"x": 3, "y": 4})
  - передається по референції (як об'єкт)
  - поля мутуються через .set_field() / __setattr__
  - копія через .copy() — явна, не випадкова
  - repr людяний: Point{x: 3, y: 4}

Reference semantics (як в Go/Rust &mut):
  - struct передається по референції завжди
  - self в методах — та сама референція, мутації видно зовні
  - явна копія тільки через copy()
"""


class OberihStruct:
    __slots__ = ("_type_name", "_fields")

    def __init__(self, type_name: str, fields: dict):
        object.__setattr__(self, "_type_name", type_name)
        object.__setattr__(self, "_fields", dict(fields))  # власний dict

    # --- доступ до полів ---

    def get_field(self, name: str):
        fields = object.__getattribute__(self, "_fields")
        if name not in fields:
            type_name = object.__getattribute__(self, "_type_name")
            raise AttributeError(
                f"Поле '{name}' не існує в структурі {type_name}"
            )
        return fields[name]

    def set_field(self, name: str, value):
        fields = object.__getattribute__(self, "_fields")
        if name not in fields:
            type_name = object.__getattribute__(self, "_type_name")
            raise AttributeError(
                f"Поле '{name}' не існує в структурі {type_name}. "
                f"Доступні поля: {list(fields.keys())}"
            )
        fields[name] = value

    def has_field(self, name: str) -> bool:
        fields = object.__getattribute__(self, "_fields")
        return name in fields

    def field_names(self):
        fields = object.__getattribute__(self, "_fields")
        return list(fields.keys())

    @property
    def type_name(self):
        return object.__getattribute__(self, "_type_name")

    # --- копія (явна) ---

    def copy(self):
        """Поверхнева копія — нова структура з тими ж значеннями полів."""
        fields = object.__getattribute__(self, "_fields")
        type_name = object.__getattribute__(self, "_type_name")
        return OberihStruct(type_name, dict(fields))

    # --- repr ---

    def __repr__(self):
        fields = object.__getattribute__(self, "_fields")
        type_name = object.__getattribute__(self, "_type_name")
        field_strs = ", ".join(f"{k}: {v!r}" for k, v in fields.items())
        return f"{type_name}{{{field_strs}}}"

    def __eq__(self, other):
        if isinstance(other, OberihStruct):
            return (
                object.__getattribute__(self, "_type_name")
                == object.__getattribute__(other, "_type_name")
                and object.__getattribute__(self, "_fields")
                == object.__getattribute__(other, "_fields")
            )
        return NotImplemented

//! Builtin library services. Python code remains compiled and executed by tis.
mod codecs;
pub mod decimal;
pub mod iterators;
pub mod pickle;
pub mod random;
pub mod regex;
use crate::{Error, Object, Result, Value, Vm};
use sha2::{Digest, Sha512};

#[derive(Debug, Clone)]
pub enum Payload {
    Date(pickle::Date),
    Attributes(Value),
    Decimal(decimal::Decimal),
    DecimalContext(decimal::Context),
    DecimalTuple {
        sign: bool,
        digits: Value,
        exponent: i64,
    },
    Random(random::Mt19937),
    Regex(regex::Pattern),
    Match(regex::Match),
    Iterator(iterators::State),
}
impl Payload {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::Date(_) => "datetime.date",
            Self::Attributes(_) => "dict",
            Self::Decimal(_) => "decimal.Decimal",
            Self::DecimalContext(_) => "decimal.Context",
            Self::DecimalTuple { .. } => "decimal.DecimalTuple",
            Self::Random(_) => "random.Random",
            Self::Regex(_) => "re.Pattern",
            Self::Match(_) => "re.Match",
            Self::Iterator(state) => state.name(),
        }
    }
    pub(crate) fn method(&self, name: &str) -> Option<&'static str> {
        if matches!(self, Self::Date(_)) {
            return None;
        }
        if matches!(self, Self::Attributes(_)) {
            return match name {
                "update" => Some("update"),
                "get" => Some("get"),
                "copy" => Some("copy"),
                _ => None,
            };
        }
        if matches!(self, Self::Decimal(_)) {
            return if name == "as_tuple" {
                Some("as_tuple")
            } else {
                None
            };
        }
        if matches!(self, Self::DecimalContext(_) | Self::DecimalTuple { .. }) {
            return None;
        }
        if matches!(self, Self::Regex(_)) {
            return match name {
                "search" => Some("search"),
                "match" => Some("match"),
                "findall" => Some("findall"),
                "sub" => Some("sub"),
                _ => None,
            };
        }
        if matches!(self, Self::Match(_)) {
            return if name == "group" { Some("group") } else { None };
        }
        if matches!(self, Self::Iterator(_)) {
            return match name {
                "__iter__" => Some("__iter__"),
                "__next__" => Some("__next__"),
                _ => None,
            };
        }
        match name {
            "random" => Some("random"),
            "seed" => Some("seed"),
            "getrandbits" => Some("getrandbits"),
            "randbytes" => Some("randbytes"),
            _ => None,
        }
    }
    pub(crate) fn visit(&self, visit: &mut impl FnMut(Value)) {
        if let Self::Attributes(owner) = self {
            visit(*owner);
        }
        if let Self::DecimalTuple { digits, .. } = self {
            visit(*digits);
        }
        if let Self::Match(m) = self {
            visit(m.subject);
        }
        if let Self::Iterator(state) = self {
            state.visit(visit);
        }
    }
}
pub(crate) fn supports_module(name: &str) -> bool {
    matches!(
        name,
        "math"
            | "json"
            | "bisect"
            | "random"
            | "base64"
            | "re"
            | "decimal"
            | "copy"
            | "dataclasses"
            | "datetime"
            | "pickle"
            | "itertools"
            | "collections"
            | "collections.abc"
    )
}
fn invalid(message: &str) -> Error {
    Error::Runtime(format!("ValueError: {message}"))
}
fn arity(args: &[Value], min: usize, max: usize) -> Result<()> {
    if (min..=max).contains(&args.len()) {
        Ok(())
    } else {
        Err(Error::Type("wrong number of library arguments"))
    }
}
impl Vm {
    pub(crate) fn library_attribute(&mut self, module: &str, name: &str) -> Result<Value> {
        if module == "pickle" && name == "HIGHEST_PROTOCOL" {
            return Ok(Value::Int(5));
        }
        let exported = match module {
            "datetime" => name == "date",
            "pickle" => matches!(name, "dumps" | "loads" | "_dumps" | "_loads"),
            "random" => matches!(
                name,
                "Random" | "random" | "seed" | "getrandbits" | "randbytes"
            ),
            "base64" => matches!(
                name,
                "b64encode"
                    | "b64decode"
                    | "urlsafe_b64encode"
                    | "urlsafe_b64decode"
                    | "b32encode"
                    | "b32decode"
                    | "b16encode"
                    | "b16decode"
                    | "a85encode"
                    | "a85decode"
                    | "b85encode"
                    | "b85decode"
            ),
            "copy" => name == "deepcopy",
            "dataclasses" => name == "dataclass",
            "decimal" => matches!(name, "Decimal" | "localcontext" | "getcontext"),
            "re" => matches!(name, "compile" | "search" | "match" | "findall" | "sub"),
            "itertools" => matches!(name, "count" | "islice"),
            "collections.abc" => name == "Iterator",
            "collections" => name == "abc",
            _ => false,
        };
        if !exported {
            return Err(Error::Runtime(format!("AttributeError: {module}.{name}")));
        }
        let key = format!("{module}.{name}");
        if let Some(value) = self.modules.get(&key) {
            return Ok(*value);
        }
        let value = self.heap.allocate(Object::Native(key.clone().into()));
        self.modules.insert(key.clone(), value);
        if matches!(
            key.as_str(),
            "random.Random" | "datetime.date" | "decimal.Decimal" | "decimal.Context"
        ) {
            self.modules.insert(format!("type.{key}"), value);
        }
        Ok(value)
    }
    pub(crate) fn library_object_attribute(
        &mut self,
        receiver: Value,
        name: &str,
    ) -> Result<Value> {
        let Object::Library(payload) = self.heap.get(receiver)? else {
            return Err(Error::Type("library value"));
        };
        match (&**payload, name) {
            (Payload::Date(date), "year") => return Ok(Value::Int(i64::from(date.year))),
            (Payload::Date(date), "month") => return Ok(Value::Int(i64::from(date.month))),
            (Payload::Date(date), "day") => return Ok(Value::Int(i64::from(date.day))),
            (Payload::DecimalContext(context), "prec") => {
                return Ok(Value::Int(context.precision as i64));
            }
            (Payload::DecimalTuple { sign, .. }, "sign") => {
                return Ok(Value::Int(i64::from(*sign)));
            }
            (Payload::DecimalTuple { digits, .. }, "digits") => return Ok(*digits),
            (Payload::DecimalTuple { exponent, .. }, "exponent") => {
                return Ok(Value::Int(*exponent));
            }
            _ => {}
        }
        if let Some(method) = payload.method(name) {
            return Ok(self.heap.allocate(Object::Bound(receiver, method.into())));
        }
        Err(Error::Runtime(format!("AttributeError: {name}")))
    }
    pub(crate) fn library_builtin(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Option<Result<Value>> {
        if !name.starts_with("datetime.")
            && !name.starts_with("pickle.")
            && !name.starts_with("copy.")
            && !name.starts_with("dataclasses.")
            && !name.starts_with("decimal.")
            && !name.starts_with("re.")
            && !name.starts_with("random.")
            && !name.starts_with("base64.")
            && !name.starts_with("itertools.")
            && !matches!(name, "map" | "filter")
        {
            return None;
        }
        Some(self.protocol_roots(args, |vm| {
            if name == "datetime.date" {
                vm.library_date(args, keywords)
            } else if let Some(name) = name.strip_prefix("pickle.") {
                vm.library_pickle(name, args, keywords)
            } else if name == "copy.deepcopy" {
                vm.library_copy(args, keywords)
            } else if let Some(name) = name.strip_prefix("dataclasses.") {
                vm.library_dataclass(name, args, keywords)
            } else if let Some(name) = name.strip_prefix("decimal.") {
                vm.library_decimal(name, args, keywords)
            } else if let Some(name) = name.strip_prefix("re.") {
                vm.library_regex(None, name, args, keywords)
            } else if name.starts_with("itertools.") || matches!(name, "map" | "filter") {
                vm.library_iterator(name, args)
            } else if let Some(name) = name.strip_prefix("random.") {
                vm.library_random(None, name, args, keywords)
            } else {
                vm.library_codec(name.strip_prefix("base64.").unwrap(), args, keywords)
            }
        }))
    }
    pub(crate) fn library_method(
        &mut self,
        receiver: Value,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if let Ok(Object::Library(p)) = self.heap.get(receiver)
            && let Payload::Attributes(owner) = &**p
        {
            let owner = *owner;
            if !keywords.is_empty() {
                return Err(Error::Unsupported("instance dictionary method keywords"));
            }
            return self.attribute_mapping_method(owner, name, args);
        }
        if self.is_decimal(receiver) {
            if !keywords.is_empty() || name != "as_tuple" {
                return Err(Error::Unsupported("Decimal method"));
            }
            return self.decimal_tuple(receiver, args);
        }
        if matches!(self.heap.get(receiver), Ok(Object::Library(p)) if matches!(&**p, Payload::Regex(_) | Payload::Match(_)))
        {
            return self.protocol_roots(&[receiver], |vm| {
                vm.library_regex(Some(receiver), name, args, keywords)
            });
        }
        if self.is_library_iterator(receiver) {
            if !keywords.is_empty() {
                return Err(Error::Type("iterator methods take no keyword arguments"));
            }
            if name == "__iter__" {
                arity(args, 0, 0)?;
                return Ok(receiver);
            }
            if name == "__next__" {
                arity(args, 0, 0)?;
                let value = self.library_next(receiver)?;
                return if self.is_done(value) {
                    Err(Error::Runtime("StopIteration".into()))
                } else {
                    Ok(value)
                };
            }
        }
        let mut roots = vec![receiver];
        roots.extend_from_slice(args);
        self.protocol_roots(&roots, |vm| {
            vm.library_random(Some(receiver), name, args, keywords)
        })
    }
    fn random_seed(&self, value: Option<Value>) -> Result<Vec<u32>> {
        match value {
            None | Some(Value::None) => {
                let mut bytes = vec![0u8; 624 * 4];
                getrandom::fill(&mut bytes)
                    .map_err(|e| Error::Runtime(format!("OSError: random entropy: {e}")))?;
                Ok(bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect())
            }
            Some(v) if crate::integer::is_integer(&self.heap, v) => {
                Ok(crate::integer::get(&self.heap, v)?.to_u32_digits().1)
            }
            Some(v) => {
                let data = if let Some(s) = self.heap.as_str(&v) {
                    s.as_bytes().to_vec()
                } else if let Ok(Object::Bytes(data)) = self.heap.get(v) {
                    data.clone()
                } else {
                    return Err(Error::Type("seed must be integer, str, bytes or None"));
                };
                let hash = Sha512::digest(&data);
                let mut data = data;
                data.extend_from_slice(&hash);
                Ok(num_bigint::BigUint::from_bytes_be(&data).to_u32_digits())
            }
        }
    }
    fn library_random(
        &mut self,
        receiver: Option<Value>,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        if !keywords.is_empty() {
            return Err(Error::Unsupported("random keyword arguments"));
        }
        if name == "Random" || name == "seed" {
            arity(args, 0, 1)?;
            let state = random::Mt19937::new(&self.random_seed(args.first().copied())?);
            if name == "Random" {
                return Ok(self
                    .heap
                    .allocate(Object::Library(Box::new(Payload::Random(state)))));
            }
            if let Some(receiver) = receiver {
                let mut object = self.heap.get_mut(receiver)?;
                let Object::Library(payload) = &mut *object else {
                    return Err(Error::Type("Random value"));
                };
                **payload = Payload::Random(state);
            } else {
                self.random_state = Some(state);
            }
            return Ok(Value::None);
        }
        let count = match name {
            "random" => {
                arity(args, 0, 0)?;
                0
            }
            "getrandbits" | "randbytes" => {
                arity(args, 1, 1)?;
                usize::try_from(self.py_index(args[0])?)
                    .map_err(|_| invalid("negative random count"))?
            }
            _ => return Err(Error::Unsupported("random operation")),
        };
        enum Output {
            Float(f64),
            Integer(num_bigint::BigInt),
            Bytes(Vec<u8>),
        }
        let generate = |state: &mut random::Mt19937| match name {
            "random" => Output::Float(state.random()),
            "getrandbits" => Output::Integer(state.bits(count)),
            _ => Output::Bytes(state.bytes(count)),
        };
        let output = if let Some(receiver) = receiver {
            let mut object = self.heap.get_mut(receiver)?;
            let Object::Library(payload) = &mut *object else {
                return Err(Error::Type("Random value"));
            };
            let Payload::Random(state) = &mut **payload else {
                return Err(Error::Type("Random value"));
            };
            generate(state)
        } else {
            if self.random_state.is_none() {
                self.random_state = Some(random::Mt19937::new(&self.random_seed(None)?));
            }
            generate(self.random_state.as_mut().unwrap())
        };
        Ok(match output {
            Output::Float(n) => Value::Float(n),
            Output::Integer(n) => crate::integer::value(&self.heap, n),
            Output::Bytes(data) => self.heap.allocate(Object::Bytes(data)),
        })
    }
    fn library_codec(
        &mut self,
        name: &str,
        args: &[Value],
        keywords: &[(String, Value)],
    ) -> Result<Value> {
        arity(args, 1, 2)?;
        let decoding = name.ends_with("decode");
        let data = match self.heap.get(args[0]) {
            Ok(Object::Bytes(data)) => data.clone(),
            _ if decoding => self
                .heap
                .as_str(&args[0])
                .filter(|s| s.is_ascii())
                .ok_or(Error::Type("expected ASCII str or bytes"))?
                .as_bytes()
                .to_vec(),
            _ => return Err(Error::Type("expected bytes")),
        };
        let (mut pad, mut casefold, mut validate) = (false, false, false);
        if let Some(&arg) = args.get(1) {
            if name.starts_with("b16") || name.starts_with("b32") {
                casefold = self.py_truth(arg)?;
            } else if name.starts_with("b85") {
                pad = self.py_truth(arg)?;
            } else {
                return Err(Error::Unsupported("codec positional argument"));
            }
        }
        for (key, &value) in keywords.iter().map(|(k, v)| (k, v)) {
            match key.as_str() {
                "pad" => pad = self.py_truth(value)?,
                "casefold" => casefold = self.py_truth(value)?,
                "validate" => validate = self.py_truth(value)?,
                _ => return Err(Error::Unsupported("codec keyword")),
            }
        }
        let output = if decoding {
            codecs::decode(name, &data, casefold, validate)?
        } else {
            codecs::encode(name, &data, pad)?
        };
        Ok(self.heap.allocate(Object::Bytes(output)))
    }
}

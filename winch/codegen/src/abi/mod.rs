//!
//! # Default ABI
//!
//! Winch uses a default internal ABI, for all internal functions.
//! This allows us to push the complexity of system ABI compliance to
//! the trampolines (not yet implemented).  The default ABI treats all
//! allocatable registers as caller saved, which means that (i) all
//! register values in the Wasm value stack (which are normally
//! referred to as "live"), must be saved onto the machine stack (ii)
//! function prologues and epilogues don't store/restore other
//! registers more than the non-allocatable ones (e.g. rsp/rbp in
//! x86_64).
//!
//! The calling convention in the default ABI, uses registers to a
//! certain fixed count for arguments and return values, and then the
//! stack is used for all additional arguments.
//!
//! Generally the stack layout looks like:
//! +-------------------------------+
//! |                               |
//! |                               |
//! |         Stack Args            |
//! |                               |
//! |                               |
//! +-------------------------------+----> SP @ function entry
//! |         Ret addr              |
//! +-------------------------------+
//! |            SP                 |
//! +-------------------------------+----> SP @ Function prologue
//! |                               |
//! |                               |
//! |                               |
//! |        Stack slots            |
//! |        + `VMContext` slot     |
//! |        + dynamic space        |
//! |                               |
//! |                               |
//! |                               |
//! +-------------------------------+----> SP @ callsite (after)
//! |        alignment              |
//! |        + arguments            |
//! |                               | ----> Space allocated for calls
//! |                               |
use crate::codegen::ptr_type_from_ptr_size;
use crate::isa::{reg::Reg, CallingConvention};
use crate::masm::{OperandSize, SPOffset};
use smallvec::SmallVec;
use std::collections::HashSet;
use std::ops::{Add, BitAnd, Not, Sub};
use wasmtime_environ::{WasmFuncType, WasmHeapType, WasmRefType, WasmValType};

pub(crate) mod local;
pub(crate) use local::*;

/// Internal classification for params or returns,
/// mainly used for params and return register assignment.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(super) enum ParamsOrReturns {
    Params,
    Returns,
}

/// Trait implemented by a specific ISA and used to provide
/// information about alignment, parameter passing, usage of
/// specific registers, etc.
pub(crate) trait ABI {
    /// The required stack alignment.
    fn stack_align() -> u8;

    /// The required stack alignment for calls.
    fn call_stack_align() -> u8;

    /// The offset to the argument base, relative to the frame pointer.
    fn arg_base_offset() -> u8;

    /// The offset to the return address, relative to the frame pointer.
    fn ret_addr_offset() -> u8;

    /// Construct the ABI-specific signature from a WebAssembly
    /// function type.
    fn sig(wasm_sig: &WasmFuncType, call_conv: &CallingConvention) -> ABISig;

    /// Construct an ABI signature from WasmType params and returns.
    fn sig_from(
        params: &[WasmValType],
        returns: &[WasmValType],
        call_conv: &CallingConvention,
    ) -> ABISig;

    /// Construct [`ABIResults`] from a slice of [`WasmType`].
    fn abi_results(returns: &[WasmValType], call_conv: &CallingConvention) -> ABIResults;

    /// Returns the number of bits in a word.
    fn word_bits() -> u8;

    /// Returns the number of bytes in a word.
    fn word_bytes() -> u8 {
        Self::word_bits() / 8
    }

    /// Returns the designated general purpose scratch register.
    fn scratch_reg() -> Reg;

    /// Returns the designated floating point scratch register.
    fn float_scratch_reg() -> Reg;

    /// Returns the designated scratch register for the given [WasmType].
    fn scratch_for(ty: &WasmValType) -> Reg {
        match ty {
            WasmValType::I32
            | WasmValType::I64
            | WasmValType::Ref(WasmRefType {
                heap_type: WasmHeapType::Func,
                ..
            }) => Self::scratch_reg(),
            WasmValType::F32 | WasmValType::F64 => Self::float_scratch_reg(),
            _ => unimplemented!(),
        }
    }

    /// Returns the frame pointer register.
    fn fp_reg() -> Reg;

    /// Returns the stack pointer register.
    fn sp_reg() -> Reg;

    /// Returns the pinned register used to hold
    /// the `VMContext`.
    fn vmctx_reg() -> Reg;

    /// Returns the callee-saved registers for the given
    /// calling convention.
    fn callee_saved_regs(call_conv: &CallingConvention) -> SmallVec<[(Reg, OperandSize); 18]>;

    /// The size, in bytes, of each stack slot used for stack parameter passing.
    fn stack_slot_size() -> u8;

    /// Returns the size in bytes of the given [`WasmType`].
    fn sizeof(ty: &WasmValType) -> u8;

    /// Returns the size in bits of the given [`WasmType`].
    fn sizeof_bits(ty: &WasmValType) -> u8;
}

/// ABI-specific representation of function argument or result.
#[derive(Clone, Debug)]
pub enum ABIOperand {
    /// A register [`ABIOperand`].
    Reg {
        /// The type of the [`ABIOperand`].
        ty: WasmValType,
        /// Register holding the [`ABIOperand`].
        reg: Reg,
        /// The size of the [`ABIOperand`], in bytes.
        size: u32,
    },
    /// A stack [`ABIOperand`].
    Stack {
        /// The type of the [`ABIOperand`].
        ty: WasmValType,
        /// Offset of the operand referenced through FP by the callee and
        /// through SP by the caller.
        offset: u32,
        /// The size of the [`ABIOperand`], in bytes.
        size: u32,
    },
}

impl ABIOperand {
    /// Allocate a new register [`ABIOperand`].
    pub fn reg(reg: Reg, ty: WasmValType, size: u32) -> Self {
        Self::Reg { reg, ty, size }
    }

    /// Allocate a new stack [`ABIOperand`].
    pub fn stack_offset(offset: u32, ty: WasmValType, size: u32) -> Self {
        Self::Stack { ty, offset, size }
    }

    /// Is this [`ABIOperand`] in a register.
    pub fn is_reg(&self) -> bool {
        match *self {
            ABIOperand::Reg { .. } => true,
            _ => false,
        }
    }

    /// Unwraps the underlying register if it is one.
    ///
    /// # Panics
    /// This function panics if the [`ABIOperand`] is not a register.
    pub fn unwrap_reg(&self) -> Reg {
        match self {
            ABIOperand::Reg { reg, .. } => *reg,
            _ => unreachable!(),
        }
    }

    /// Get the register associated to this [`ABIOperand`].
    pub fn get_reg(&self) -> Option<Reg> {
        match *self {
            ABIOperand::Reg { reg, .. } => Some(reg),
            _ => None,
        }
    }

    /// Get the type associated to this [`ABIOperand`].
    pub fn ty(&self) -> WasmValType {
        match *self {
            ABIOperand::Reg { ty, .. } | ABIOperand::Stack { ty, .. } => ty,
        }
    }
}

/// Information about the [`ABIOperand`] information used in [`ABISig`].
#[derive(Clone, Debug)]
pub(crate) struct ABIOperands {
    /// All the operands.
    pub inner: SmallVec<[ABIOperand; 6]>,
    /// All the registers used as operands.
    pub regs: HashSet<Reg>,
    /// Stack bytes used by the operands.
    pub bytes: u32,
}

impl Default for ABIOperands {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            regs: HashSet::with_capacity(0),
            bytes: 0,
        }
    }
}

/// Machine stack location of the stack results.
#[derive(Debug, Copy, Clone)]
pub(crate) enum RetArea {
    /// Addressed from the stack pointer at the given offset.
    SP(SPOffset),
    /// The address of the results base is stored at a particular,
    /// well known [LocalSlot].
    Slot(LocalSlot),
    /// The return area cannot be fully resolved ahead-of-time.
    /// If there are results on the stack, this is the default state to which
    /// all return areas get initialized to until they can be fully resolved to
    /// either a [RetArea::SP] or [RetArea::Slot].
    ///
    /// This allows a more explicit differentiation between the existence of
    /// a return area versus no return area at all.
    Uninit,
}

impl Default for RetArea {
    fn default() -> Self {
        Self::Uninit
    }
}

impl RetArea {
    /// Create a [RetArea] addressed from SP at the given offset.
    pub fn sp(offs: SPOffset) -> Self {
        Self::SP(offs)
    }

    /// Create a [RetArea] addressed stored at the given [LocalSlot].
    pub fn slot(local: LocalSlot) -> Self {
        Self::Slot(local)
    }

    /// Returns the [SPOffset] used as the base of the return area.
    ///
    /// # Panics
    /// This function panics if the return area doesn't hold a [SPOffset].
    pub fn unwrap_sp(&self) -> SPOffset {
        match self {
            Self::SP(offs) => *offs,
            _ => unreachable!(),
        }
    }

    /// Returns true if the return area is addressed via the stack pointer.
    pub fn is_sp(&self) -> bool {
        match self {
            Self::SP(_) => true,
            _ => false,
        }
    }

    /// Returns true if the return area is uninitiliazed.
    pub fn is_uninit(&self) -> bool {
        match self {
            Self::Uninit => true,
            _ => false,
        }
    }
}

/// ABI-specific representation of an [`ABISig`].
#[derive(Clone, Debug, Default)]
pub(crate) struct ABIResults {
    /// The result operands.
    operands: ABIOperands,
    /// The return area, if there are results on the stack.
    ret_area: Option<RetArea>,
}

impl ABIResults {
    /// Creates [`ABIResults`] from a slice of `WasmType`.
    /// This function maps the given return types to their ABI specific
    /// representation. It does so, by iterating over them and applying the
    /// given `map` closure. The map closure takes a [WasmType], maps its ABI
    /// representation, according to the calling convention. In the case of
    /// results, one result is stored in registers and the rest at particular
    /// offsets in the stack.
    pub fn from<F>(returns: &[WasmValType], call_conv: &CallingConvention, mut map: F) -> Self
    where
        F: FnMut(&WasmValType, u32) -> (ABIOperand, u32),
    {
        if returns.len() == 0 {
            return Self::default();
        }

        type FoldTuple = (SmallVec<[ABIOperand; 6]>, HashSet<Reg>, u32);

        let fold_impl = |(mut operands, mut regs, stack_bytes): FoldTuple, arg| {
            let (operand, bytes) = map(arg, stack_bytes);
            if operand.is_reg() {
                regs.insert(operand.unwrap_reg());
            }
            operands.push(operand);
            (operands, regs, bytes)
        };

        // When dealing with multiple results, Winch's calling convention stores the
        // last return value in a register rather than the first one. In that
        // sense, Winch's return values in the ABI signature are "reversed" in
        // terms of storage. This technique is particularly helpful to ensure that
        // the following invariants are maintained:
        // * Spilled memory values always precede register values
        // * Spilled values are stored from oldest to newest, matching their
        //   respective locations on the machine stack.
        let (mut operands, regs, bytes): FoldTuple = if call_conv.is_default() {
            returns
                .iter()
                .rev()
                .fold((SmallVec::new(), HashSet::with_capacity(1), 0), fold_impl)
        } else {
            returns
                .iter()
                .fold((SmallVec::new(), HashSet::with_capacity(1), 0), fold_impl)
        };

        // Similar to above, we reverse the result of the operands calculation
        // to ensure that they match the declared order.
        if call_conv.is_default() {
            operands.reverse();
        }

        Self::new(ABIOperands {
            inner: operands,
            regs,
            bytes,
        })
    }

    /// Create a new [`ABIResults`] from [`ABIOperands`].
    pub fn new(operands: ABIOperands) -> Self {
        let ret_area = (operands.bytes > 0).then(|| RetArea::default());
        Self { operands, ret_area }
    }

    /// Returns a reference to a [HashSet<Reg>], which includes
    /// all the registers used to hold function results.
    pub fn regs(&self) -> &HashSet<Reg> {
        &self.operands.regs
    }

    /// Get a slice over all the result [`ABIOperand`]s.
    pub fn operands(&self) -> &[ABIOperand] {
        &self.operands.inner
    }

    /// Returns the length of the result.
    pub fn len(&self) -> usize {
        self.operands.inner.len()
    }

    /// Returns the length of results on the stack.
    pub fn stack_operands_len(&self) -> usize {
        self.operands().len() - self.regs().len()
    }

    /// Get the [`ABIOperand`] result in the nth position.
    #[cfg(test)]
    pub fn get(&self, n: usize) -> Option<&ABIOperand> {
        self.operands.inner.get(n)
    }

    /// Returns the first [`ABIOperand`].
    /// Useful in situations where the function signature is known to
    /// have a single return.
    ///
    /// # Panics
    /// This function panics if the function signature contains more
    pub fn unwrap_singleton(&self) -> &ABIOperand {
        debug_assert_eq!(self.len(), 1);
        &self.operands.inner[0]
    }

    /// Returns the size, in bytes of all the [`ABIOperand`]s in the stack.
    pub fn size(&self) -> u32 {
        self.operands.bytes
    }

    /// Returns true if the [`ABIResults`] require space on the machine stack
    /// for results.
    pub fn on_stack(&self) -> bool {
        self.operands.bytes > 0
    }

    /// Set the return area of the signature.
    ///
    /// # Panics
    ///
    /// This function will panic if trying to set a return area if there are
    /// no results on the stack or if trying to set an uninitialize return area.
    /// This method must only be used when the return area can be fully
    /// materialized.
    pub fn set_ret_area(&mut self, area: RetArea) {
        debug_assert!(self.on_stack());
        debug_assert!(!area.is_uninit());
        self.ret_area = Some(area);
    }

    /// Returns a reference to the return area, if any.
    pub fn ret_area(&self) -> Option<&RetArea> {
        self.ret_area.as_ref()
    }
}

/// ABI-specific representation of an [`ABISig`].
#[derive(Debug, Clone, Default)]
pub(crate) struct ABIParams {
    /// The param operands.
    operands: ABIOperands,
    /// Whether [`ABIParams`] contains an extra paramter for the stack
    /// result area.
    has_retptr: bool,
}

impl ABIParams {
    /// Creates [`ABIParams`] from a slice of `WasmType`.
    /// This function maps the given param types to their ABI specific
    /// representation. It does so, by iterating over them and applying the
    /// given `map` closure. The map closure takes a [WasmType], maps its ABI
    /// representation, according to the calling convention. In the case of
    /// params, multiple params may be passed in registers and the rest on the
    /// stack depending on the calling convention.
    pub fn from<F, A: ABI>(
        params: &[WasmValType],
        initial_bytes: u32,
        needs_stack_results: bool,
        mut map: F,
    ) -> Self
    where
        F: FnMut(&WasmValType, u32) -> (ABIOperand, u32),
    {
        if params.len() == 0 && !needs_stack_results {
            return Self::with_bytes(initial_bytes);
        }

        let regiser_capacity = params.len().min(6);
        let (mut operands, mut regs, mut stack_bytes): (
            SmallVec<[ABIOperand; 6]>,
            HashSet<Reg>,
            u32,
        ) = params.iter().fold(
            (
                SmallVec::new(),
                HashSet::with_capacity(regiser_capacity),
                initial_bytes,
            ),
            |(mut operands, mut regs, stack_bytes), arg| {
                let (operand, bytes) = map(arg, stack_bytes);
                if operand.is_reg() {
                    regs.insert(operand.unwrap_reg());
                }
                operands.push(operand);
                (operands, regs, bytes)
            },
        );

        let ptr_type = ptr_type_from_ptr_size(<A as ABI>::word_bytes());
        // Handle stack results by specifying an extra, implicit last argument.
        if needs_stack_results {
            let (operand, bytes) = map(&ptr_type, stack_bytes);
            if operand.is_reg() {
                regs.insert(operand.unwrap_reg());
            }
            operands.push(operand);
            stack_bytes = bytes;
        }

        Self {
            operands: ABIOperands {
                inner: operands,
                regs,
                bytes: stack_bytes,
            },
            has_retptr: needs_stack_results,
        }
    }

    /// Creates new [`ABIParams`], with the specified amount of stack bytes.
    pub fn with_bytes(bytes: u32) -> Self {
        let mut params = Self::default();
        params.operands.bytes = bytes;
        params
    }

    /// Get the [`ABIOperand`] param in the nth position.
    pub fn get(&self, n: usize) -> Option<&ABIOperand> {
        self.operands.inner.get(n)
    }

    /// Get a slice over all the parameter [`ABIOperand`]s.
    pub fn operands(&self) -> &[ABIOperand] {
        &self.operands.inner
    }

    /// Returns the length of the params, including the return pointer,
    /// if any.
    pub fn len(&self) -> usize {
        self.operands.inner.len()
    }

    /// Returns the length of the params, excluding the return pointer,
    /// if any.
    pub fn len_without_retptr(&self) -> usize {
        if self.has_retptr {
            self.len() - 1
        } else {
            self.len()
        }
    }

    /// Returns true if the [ABISig] has an extra parameter for stack results.
    pub fn has_retptr(&self) -> bool {
        self.has_retptr
    }

    /// Returns the last [ABIOperand] used as the pointer to the
    /// stack results area.
    ///
    /// # Panics
    /// This function panics if the [ABIParams] doesn't have a stack results
    /// parameter.
    pub fn unwrap_results_area_operand(&self) -> &ABIOperand {
        debug_assert!(self.has_retptr);
        self.operands.inner.last().unwrap()
    }
}

/// An ABI-specific representation of a function signature.
#[derive(Debug, Clone, Default)]
pub(crate) struct ABISig {
    /// Function parameters.
    pub params: ABIParams,
    /// Function result.
    pub results: ABIResults,
    /// A unique set of registers used in the entire [`ABISig`].
    pub regs: HashSet<Reg>,
}

impl ABISig {
    /// Create a new ABI signature.
    pub fn new(params: ABIParams, results: ABIResults) -> Self {
        let regs = params
            .operands
            .regs
            .union(&results.operands.regs)
            .copied()
            .collect();
        Self {
            params,
            results,
            regs,
        }
    }

    /// Returns an iterator over all the parameter operands.
    pub fn params(&self) -> &[ABIOperand] {
        self.params.operands()
    }

    /// Returns an iterator over all the result operands.
    pub fn results(&self) -> &[ABIOperand] {
        self.results.operands()
    }

    /// Returns a slice over the signature params, excluding the results
    /// base paramter, if any.
    pub fn params_without_retptr(&self) -> &[ABIOperand] {
        if self.params.has_retptr() {
            &self.params()[0..(self.params.len() - 1)]
        } else {
            self.params()
        }
    }

    /// Returns the stack size, in bytes, needed for arguments on the stack.
    pub fn params_stack_size(&self) -> u32 {
        self.params.operands.bytes
    }

    /// Returns the stack size, in bytes, needed for results on the stack.
    pub fn results_stack_size(&self) -> u32 {
        self.results.operands.bytes
    }

    /// Returns true if the signature has results on the stack.
    pub fn has_stack_results(&self) -> bool {
        self.results.on_stack()
    }
}

/// Align a value up to the given power-of-two-alignment.
// See https://sites.google.com/site/theoryofoperatingsystems/labs/malloc/align8
pub(crate) fn align_to<N>(value: N, alignment: N) -> N
where
    N: Not<Output = N>
        + BitAnd<N, Output = N>
        + Add<N, Output = N>
        + Sub<N, Output = N>
        + From<u8>
        + Copy,
{
    let alignment_mask = alignment - 1.into();
    (value + alignment_mask) & !alignment_mask
}

/// Calculates the delta needed to adjust a function's frame plus some
/// addend to a given alignment.
pub(crate) fn calculate_frame_adjustment(frame_size: u32, addend: u32, alignment: u32) -> u32 {
    let total = frame_size + addend;
    (alignment - (total % alignment)) % alignment
}

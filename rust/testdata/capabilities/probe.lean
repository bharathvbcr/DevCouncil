import Init.Data.BitVec

namespace Signal.Spec

structure BaseFrame where
  epoch : Nat

structure HandshakeFrame extends BaseFrame where
  nonce : Nat

def stepEpoch (frame : HandshakeFrame) : Nat :=
  frame.epoch

theorem stepEpoch_eq (frame : HandshakeFrame) : stepEpoch frame = frame.epoch := by
  rfl

end Signal.Spec

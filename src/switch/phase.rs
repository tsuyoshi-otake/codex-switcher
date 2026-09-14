use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPhase {
    Idle,
    StoppingCodex,
    WaitingForExit,
    SavingCurrentCredentials,
    DecryptingTargetProfile,
    ReplacingCredentials,
    StartingCodex,
    Verifying,
    Completed,
    RollingBack,
}

impl fmt::Display for SwitchPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SwitchPhase::Idle => "idle",
            SwitchPhase::StoppingCodex => "stopping Codex",
            SwitchPhase::WaitingForExit => "waiting for Codex to exit",
            SwitchPhase::SavingCurrentCredentials => "saving current credentials",
            SwitchPhase::DecryptingTargetProfile => "decrypting target profile",
            SwitchPhase::ReplacingCredentials => "replacing credentials",
            SwitchPhase::StartingCodex => "starting Codex",
            SwitchPhase::Verifying => "verifying",
            SwitchPhase::Completed => "completed",
            SwitchPhase::RollingBack => "rolling back",
        })
    }
}

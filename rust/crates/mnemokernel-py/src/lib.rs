//! Minimal Python bridge. JSON keeps the initial ABI explicit and versioned.

use mnemokernel_core::{
    CapturePolicyRequest, DailyJournalProposal, DailyJournalRequest, PurgeScopeRequest,
    RawEventInput, RecallRequest, RetentionRequest, ScopeStatsRequest, ValidationError,
};
use mnemokernel_store::{KernelStore, StoreError};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::path::Path;

fn store_error(error: StoreError) -> PyErr {
    match error {
        StoreError::Validation(validation) => PyValueError::new_err(validation.to_string()),
        other => PyRuntimeError::new_err(other.to_string()),
    }
}

fn json_error(error: serde_json::Error) -> PyErr {
    PyValueError::new_err(format!("invalid protocol JSON: {error}"))
}

fn validation_error(error: ValidationError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

#[pyclass]
struct Kernel {
    store: Option<KernelStore>,
}

impl Kernel {
    fn store(&self) -> PyResult<&KernelStore> {
        self.store
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("native kernel is closed"))
    }
}

#[pymethods]
impl Kernel {
    #[new]
    fn new(database_path: String) -> PyResult<Self> {
        let store = KernelStore::open(Path::new(&database_path)).map_err(store_error)?;
        Ok(Self { store: Some(store) })
    }

    fn health_json(&self) -> PyResult<String> {
        let health = self.store()?.health().map_err(store_error)?;
        serde_json::to_string(&health).map_err(json_error)
    }

    fn inspect_json(&self, payload: &str) -> PyResult<String> {
        let request = serde_json::from_str(payload).map_err(json_error)?;
        let result = self.store()?.inspect(&request).map_err(store_error)?;
        serde_json::to_string(&result).map_err(json_error)
    }

    fn ingest_event_json(&self, payload: &str) -> PyResult<String> {
        let event: RawEventInput = serde_json::from_str(payload).map_err(json_error)?;
        event.validate().map_err(validation_error)?;
        let outcome = self.store()?.ingest_event(&event).map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn journal_context_json(&self, payload: &str) -> PyResult<String> {
        let request: DailyJournalRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self
            .store()?
            .journal_context(&request)
            .map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn save_journal_json(&self, request_payload: &str, proposal_payload: &str) -> PyResult<String> {
        let request: DailyJournalRequest =
            serde_json::from_str(request_payload).map_err(json_error)?;
        let proposal: DailyJournalProposal =
            serde_json::from_str(proposal_payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self
            .store()?
            .save_journal(&request, &proposal, proposal_payload)
            .map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn read_journal_json(&self, payload: &str) -> PyResult<String> {
        let request: DailyJournalRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self.store()?.read_journal(&request).map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn recall_request_json(&self, payload: &str) -> PyResult<String> {
        let request: RecallRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self
            .store()?
            .request_recall(&request, payload)
            .map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn purge_scope_json(&self, payload: &str) -> PyResult<String> {
        let request: PurgeScopeRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self.store()?.purge_scope(&request).map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn set_capture_policy_json(&self, payload: &str) -> PyResult<String> {
        let request: CapturePolicyRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self
            .store()?
            .set_capture_policy(&request)
            .map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn retain_payloads_json(&self, payload: &str) -> PyResult<String> {
        let request: RetentionRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self
            .store()?
            .retain_payloads(&request)
            .map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn scope_stats_json(&self, payload: &str) -> PyResult<String> {
        let request: ScopeStatsRequest = serde_json::from_str(payload).map_err(json_error)?;
        request.validate().map_err(validation_error)?;
        let outcome = self.store()?.scope_stats(&request).map_err(store_error)?;
        serde_json::to_string(&outcome).map_err(json_error)
    }

    fn close(&mut self) {
        self.store.take();
    }
}

#[pymodule]
fn _mnemokernel(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Kernel>()?;
    Ok(())
}

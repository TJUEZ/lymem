//! Pluggable vector index. The competition build uses Kylin Vector Engine;
//! sqlite-vec remains available for development and recovery.

use crate::error::Result;
#[cfg(feature = "kylin-vector")]
use crate::error::CoreError;

pub trait VectorIndex: Send {
    fn name(&self) -> &'static str;
    fn upsert(&mut self, id: i64, vector: &[f32]) -> Result<()>;
    fn search(&mut self, vector: &[f32], top_k: usize) -> Result<Vec<(i64, f64)>>;
    fn delete(&mut self, ids: &[i64]) -> Result<()>;
}

#[cfg(feature = "kylin-vector")]
pub struct KylinVectorIndex {
    raw: *mut std::ffi::c_void,
    dim: usize,
}

#[cfg(feature = "kylin-vector")]
unsafe impl Send for KylinVectorIndex {}

#[cfg(feature = "kylin-vector")]
impl KylinVectorIndex {
    pub fn connect(collection: &str, dim: usize) -> Result<Self> {
        use std::ffi::CString;
        let collection = CString::new(collection)
            .map_err(|_| CoreError::InvalidInput("向量集合名称包含 NUL".into()))?;
        let mut error = vec![0i8; 512];
        let raw = unsafe {
            lymem_kylin_vector_open(collection.as_ptr(), dim as i32, error.as_mut_ptr(), error.len())
        };
        if raw.is_null() {
            return Err(vector_error(&error));
        }
        Ok(Self { raw, dim })
    }
}

#[cfg(feature = "kylin-vector")]
impl VectorIndex for KylinVectorIndex {
    fn name(&self) -> &'static str {
        "kylin-vector-engine"
    }

    fn upsert(&mut self, id: i64, vector: &[f32]) -> Result<()> {
        self.check_dim(vector)?;
        let mut error = vec![0i8; 512];
        let code = unsafe {
            lymem_kylin_vector_upsert(
                self.raw,
                id,
                vector.as_ptr(),
                vector.len(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if code < 0 { Err(vector_error(&error)) } else { Ok(()) }
    }

    fn search(&mut self, vector: &[f32], top_k: usize) -> Result<Vec<(i64, f64)>> {
        self.check_dim(vector)?;
        let mut ids = vec![0i64; top_k];
        let mut scores = vec![0f32; top_k];
        let mut error = vec![0i8; 512];
        let count = unsafe {
            lymem_kylin_vector_search(
                self.raw,
                vector.as_ptr(),
                vector.len(),
                top_k,
                ids.as_mut_ptr(),
                scores.as_mut_ptr(),
                top_k,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        if count < 0 {
            return Err(vector_error(&error));
        }
        Ok(ids.into_iter().zip(scores).take(count as usize).map(|(id, score)| (id, score as f64)).collect())
    }

    fn delete(&mut self, ids: &[i64]) -> Result<()> {
        let mut error = vec![0i8; 512];
        let code = unsafe {
            lymem_kylin_vector_delete(self.raw, ids.as_ptr(), ids.len(), error.as_mut_ptr(), error.len())
        };
        if code < 0 { Err(vector_error(&error)) } else { Ok(()) }
    }
}

#[cfg(feature = "kylin-vector")]
impl KylinVectorIndex {
    fn check_dim(&self, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(CoreError::Embed(format!("向量维度不匹配: {} != {}", vector.len(), self.dim)));
        }
        Ok(())
    }
}

#[cfg(feature = "kylin-vector")]
impl Drop for KylinVectorIndex {
    fn drop(&mut self) {
        unsafe { lymem_kylin_vector_close(self.raw) };
    }
}

#[cfg(feature = "kylin-vector")]
fn vector_error(buffer: &[i8]) -> CoreError {
    let bytes: Vec<u8> = buffer.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    CoreError::Vector(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(feature = "kylin-vector")]
extern "C" {
    fn lymem_kylin_vector_open(
        collection: *const std::ffi::c_char,
        dimension: i32,
        error: *mut std::ffi::c_char,
        capacity: usize,
    ) -> *mut std::ffi::c_void;
    fn lymem_kylin_vector_upsert(
        raw: *mut std::ffi::c_void,
        id: i64,
        vector: *const f32,
        dimension: usize,
        error: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
    fn lymem_kylin_vector_search(
        raw: *mut std::ffi::c_void,
        vector: *const f32,
        dimension: usize,
        top_k: usize,
        ids: *mut i64,
        scores: *mut f32,
        capacity: usize,
        error: *mut std::ffi::c_char,
        error_capacity: usize,
    ) -> i32;
    fn lymem_kylin_vector_delete(
        raw: *mut std::ffi::c_void,
        ids: *const i64,
        count: usize,
        error: *mut std::ffi::c_char,
        capacity: usize,
    ) -> i32;
    fn lymem_kylin_vector_close(raw: *mut std::ffi::c_void);
}

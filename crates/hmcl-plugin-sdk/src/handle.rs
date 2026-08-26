use crate::abi::{HmclHandleId, HmclStatus};
use crate::{Error, ErrorCode, HandleValue, PluginContext};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

/// Associates a Rust marker type with one canonical Bridge handle type name.
pub trait HandleType: Send + Sync + 'static {
    /// Canonical lowercase type name expected in the wire handle.
    const TYPE_NAME: &'static str;
}

/// One owned reference to a host-managed object of type `T`.
pub struct ObjectHandle<T: HandleType> {
    context: PluginContext,
    value: HandleValue,
    marker: PhantomData<T>,
}

impl<T: HandleType> ObjectHandle<T> {
    /// Adopts a reference already transferred by the host without retaining it again.
    ///
    /// # Safety
    ///
    /// The caller must own exactly one live host reference represented by `value` and must not
    /// release or adopt that same reference elsewhere after this call succeeds.
    pub unsafe fn from_owned(context: &PluginContext, value: HandleValue) -> Result<Self, Error> {
        Self::validate(context, value)
    }

    /// Acquires a new owned reference from a borrowed live handle.
    pub fn from_borrowed(context: &PluginContext, value: HandleValue) -> Result<Self, Error> {
        context.ensure_handle_callbacks()?;
        validate_type::<T>(&value)?;
        context.retain_handle(HmclHandleId::from_raw(value.object_id()))?;
        Ok(Self {
            context: context.clone(),
            value,
            marker: PhantomData,
        })
    }

    /// Acquires another independently owned host reference.
    pub fn try_clone(&self) -> Result<Self, Error> {
        Self::from_borrowed(&self.context, self.value.clone())
    }

    /// Returns the validated untyped handle value.
    #[must_use]
    pub const fn value(&self) -> &HandleValue {
        &self.value
    }

    fn validate(context: &PluginContext, value: HandleValue) -> Result<Self, Error> {
        context.ensure_release_handle()?;
        validate_type::<T>(&value)?;
        Ok(Self {
            context: context.clone(),
            value,
            marker: PhantomData,
        })
    }
}

fn validate_type<T: HandleType>(value: &HandleValue) -> Result<(), Error> {
    if value.type_name() != T::TYPE_NAME {
        return Err(Error::new(ErrorCode::InvalidArgument));
    }
    Ok(())
}

impl<T: HandleType> Debug for ObjectHandle<T> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObjectHandle")
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

impl<T: HandleType> Drop for ObjectHandle<T> {
    fn drop(&mut self) {
        if self
            .context
            .release_handle(HmclHandleId::from_raw(self.value.object_id()))
            .is_err()
        {
            self.context.record_cleanup_failure();
        }
    }
}

pub(crate) fn retain_status(status: HmclStatus) -> Result<(), Error> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::new(ErrorCode::StaleHandle))
    }
}

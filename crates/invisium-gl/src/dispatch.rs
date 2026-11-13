//! `Context::call`: op validation, context loss, error recording and the method switch.

use crate::args::*;
use crate::cmd::GlDevice;
use crate::consts1::*;
use crate::tables::{self, op};
use crate::{Arg, Context, Ret, Version};

/// What a failed call returns.
fn fail_ret(name: &str) -> Ret {
    match name {
        "checkFramebufferStatus" => Ret::Num(0.0),
        "getAttribLocation" | "getFragDataLocation" => Ret::Num(-1.0),
        _ if name.starts_with("is") => Ret::Bool(false),
        _ if name.starts_with("get") || name.starts_with("create") || name == "fenceSync" => Ret::Null,
        _ => Ret::Undefined,
    }
}

impl Context {
    fn op_allowed(&self, op: u16) -> bool {
        if op == op::loseContext || op == op::restoreContext {
            return true;
        }
        if (1..=tables::methods(self.version).len()).contains(&(op as usize)) {
            return true;
        }
        if self.version == Version::WebGl1 {
            let ext_ops: [(&str, &[u16]); 3] = [
                ("ANGLE_instanced_arrays", &[op::drawArraysInstanced, op::drawElementsInstanced, op::vertexAttribDivisor]),
                ("OES_vertex_array_object", &[op::createVertexArray, op::deleteVertexArray, op::isVertexArray, op::bindVertexArray]),
                ("WEBGL_draw_buffers", &[op::drawBuffers]),
            ];
            return ext_ops.iter().any(|(e, ops)| ops.contains(&op) && self.ext(e));
        }
        false
    }

    pub(crate) fn dispatch(&mut self, dev: &mut dyn GlDevice, op: u16, a: &[Arg]) -> Ret {
        let Some(name) = tables::op_name(op) else { return Ret::Undefined };
        if !self.op_allowed(op) {
            return Ret::Undefined;
        }
        if self.lost {
            return self.lost_call(name);
        }
        match self.run(dev, name, a) {
            Ok(r) => r,
            Err(e) => {
                self.set_error(e);
                fail_ret(name)
            }
        }
    }

    fn lost_call(&mut self, name: &str) -> Ret {
        match name {
            "getError" => {
                let e = if self.lost_pending { CONTEXT_LOST_WEBGL } else { NO_ERROR };
                self.lost_pending = false;
                Ret::Num(e as f64)
            }
            "isContextLost" => Ret::Bool(true),
            "checkFramebufferStatus" => Ret::Num(FRAMEBUFFER_UNSUPPORTED as f64),
            "getUniformBlockIndex" => Ret::Num(crate::consts2::INVALID_INDEX as f64),
            "clientWaitSync" => Ret::Num(crate::consts2::WAIT_FAILED as f64),
            "restoreContext" => {
                self.set_error(INVALID_OPERATION);
                Ret::Undefined
            }
            _ if name.starts_with("is") => Ret::Bool(false),
            _ => match fail_ret(name) {
                Ret::Undefined => Ret::Undefined,
                Ret::Num(n) if n < 0.0 => Ret::Num(n),
                _ => Ret::Null,
            },
        }
    }

    fn run(&mut self, dev: &mut dyn GlDevice, name: &str, a: &[Arg]) -> R<Ret> {
        match name {
            "getParameter" | "getContextAttributes" | "getSupportedExtensions" | "getExtension"
            | "getShaderPrecisionFormat" | "getIndexedParameter" => self.query_call(name, a),
            "finish" | "flush" => {
                self.flush(dev);
                Ok(Ret::Undefined)
            }
            "getError" => {
                let e = std::mem::replace(&mut self.error, NO_ERROR);
                Ok(Ret::Num(e as f64))
            }
            "isContextLost" => Ok(Ret::Bool(false)),
            "loseContext" => {
                self.lost = true;
                self.lost_pending = true;
                Ok(Ret::Undefined)
            }
            "restoreContext" => Err(INVALID_OPERATION),
            "enable" => self.set_cap(a, true),
            "disable" => self.set_cap(a, false),
            "isEnabled" => Ok(Ret::Bool(*self.cap(uint(a, 0)?)?)),
            "blendColor" | "clearColor" | "blendEquation" | "blendEquationSeparate" | "blendFunc"
            | "blendFuncSeparate" | "clearDepth" | "clearStencil" | "colorMask" | "cullFace" | "depthFunc"
            | "depthMask" | "depthRange" | "frontFace" | "hint" | "lineWidth" | "polygonOffset"
            | "sampleCoverage" | "scissor" | "viewport" | "stencilFunc" | "stencilFuncSeparate" | "stencilMask"
            | "stencilMaskSeparate" | "stencilOp" | "stencilOpSeparate" | "activeTexture" | "pixelStorei" => {
                self.set_state(name, a)
            }
            "createBuffer" | "isBuffer" | "deleteBuffer" | "bindBuffer" | "bindBufferBase" | "bindBufferRange"
            | "bufferData" | "bufferSubData" | "getBufferParameter" | "getBufferSubData" | "copyBufferSubData" => {
                self.buffer_call(dev, name, a)
            }
            "createTexture" | "isTexture" | "deleteTexture" | "bindTexture" | "texParameteri" | "texParameterf"
            | "getTexParameter" | "generateMipmap" | "texStorage2D" | "texStorage3D" => self.texture_call(name, a),
            "texImage2D" | "texImage3D" | "texSubImage2D" | "texSubImage3D" => self.teximage_call(dev, name, a),
            "createRenderbuffer" | "isRenderbuffer" | "deleteRenderbuffer" | "bindRenderbuffer" | "renderbufferStorage"
            | "renderbufferStorageMultisample" | "getRenderbufferParameter" => self.renderbuffer_call(name, a),
            "createFramebuffer" | "isFramebuffer" | "deleteFramebuffer" | "bindFramebuffer" | "checkFramebufferStatus"
            | "framebufferTexture2D" | "framebufferRenderbuffer" | "framebufferTextureLayer"
            | "getFramebufferAttachmentParameter" | "drawBuffers" | "readBuffer" | "invalidateFramebuffer"
            | "invalidateSubFramebuffer" => self.framebuffer_call(name, a),
            "clear" | "clearBufferfv" | "clearBufferiv" | "clearBufferuiv" | "clearBufferfi" => self.clear_call(name, a),
            "readPixels" => self.read_pixels(dev, a),
            "blitFramebuffer" => self.blit(a),
            "copyTexImage2D" | "copyTexSubImage2D" | "copyTexSubImage3D" => self.copy_tex_call(dev, name, a),
            "createShader" | "isShader" | "deleteShader" | "shaderSource" | "compileShader" | "getShaderParameter"
            | "getShaderInfoLog" | "getShaderSource" | "createProgram" | "isProgram" | "deleteProgram" | "attachShader"
            | "detachShader" | "getAttachedShaders" | "bindAttribLocation" | "linkProgram" | "useProgram"
            | "validateProgram" | "getProgramInfoLog" | "getProgramParameter" | "getAttribLocation"
            | "getFragDataLocation" | "getActiveAttrib" | "getActiveUniform" | "getUniformLocation"
            | "getUniformBlockIndex" | "getActiveUniformBlockName" | "getActiveUniformBlockParameter"
            | "uniformBlockBinding" | "getUniformIndices" | "getActiveUniforms" => self.program_call(name, a),
            "getUniform" => self.get_uniform(a),
            _ if crate::uniforms::is_uniform_setter(name) => self.uniform_call(name, a),
            "createVertexArray" | "isVertexArray" | "deleteVertexArray" | "bindVertexArray" | "getVertexAttrib"
            | "getVertexAttribOffset" => self.vertex_call(name, a),
            _ if name.starts_with("vertexAttrib") || name.ends_with("VertexAttribArray") => self.vertex_call(name, a),
            "drawArrays" | "drawElements" | "drawArraysInstanced" | "drawElementsInstanced" | "drawRangeElements" => {
                self.draw_call(name, a)
            }
            "createSampler" | "isSampler" | "deleteSampler" | "bindSampler" | "samplerParameteri" | "samplerParameterf"
            | "getSamplerParameter" | "createQuery" | "isQuery" | "deleteQuery" | "beginQuery" | "endQuery" | "getQuery"
            | "getQueryParameter" | "fenceSync" | "isSync" | "deleteSync" | "clientWaitSync" | "waitSync"
            | "getSyncParameter" | "getInternalformatParameter" | "createTransformFeedback" | "isTransformFeedback"
            | "deleteTransformFeedback" | "bindTransformFeedback" | "beginTransformFeedback" | "endTransformFeedback"
            | "pauseTransformFeedback" | "resumeTransformFeedback" | "transformFeedbackVaryings"
            | "getTransformFeedbackVarying" => self.inert_call(name, a),
            "compressedTexImage2D" | "compressedTexSubImage2D" | "compressedTexImage3D" | "compressedTexSubImage3D" => {
                Err(INVALID_ENUM)
            }
            _ => Ok(Ret::Undefined),
        }
    }
}

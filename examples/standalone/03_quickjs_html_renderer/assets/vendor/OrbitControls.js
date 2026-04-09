import {
	Controls,
	MOUSE,
	Quaternion,
	Spherical,
	TOUCH,
	Vector2,
	Vector3,
	Plane,
	Ray,
	MathUtils
} from './three.webgpu.js';

const _changeEvent = { type: 'change' };
const _startEvent = { type: 'start' };
const _endEvent = { type: 'end' };

const _ray = new Ray();
const _plane = new Plane();
const _TILT_LIMIT = Math.cos( 70 * MathUtils.DEG2RAD );

const _v = new Vector3();
const _twoPI = 2 * Math.PI;

const _STATE = {
	NONE: - 1,
	ROTATE: 0,
	DOLLY: 1,
	PAN: 2,
	TOUCH_ROTATE: 3,
	TOUCH_PAN: 4,
	TOUCH_DOLLY_PAN: 5,
	TOUCH_DOLLY_ROTATE: 6
};
const _EPS = 0.000001;

class OrbitControls extends Controls {
	constructor( object, domElement = null ) {
		super( object, domElement );

		this.state = _STATE.NONE;
		this.target = new Vector3();
		this.cursor = new Vector3();
		this.minDistance = 0;
		this.maxDistance = Infinity;
		this.minZoom = 0;
		this.maxZoom = Infinity;
		this.minTargetRadius = 0;
		this.maxTargetRadius = Infinity;
		this.minPolarAngle = 0;
		this.maxPolarAngle = Math.PI;
		this.minAzimuthAngle = - Infinity;
		this.maxAzimuthAngle = Infinity;
		this.enableDamping = false;
		this.dampingFactor = 0.05;
		this.enableZoom = true;
		this.zoomSpeed = 1.0;
		this.enableRotate = true;
		this.rotateSpeed = 1.0;
		this.keyRotateSpeed = 1.0;
		this.enablePan = true;
		this.panSpeed = 1.0;
		this.screenSpacePanning = true;
		this.keyPanSpeed = 7.0;
		this.zoomToCursor = false;
		this.autoRotate = false;
		this.autoRotateSpeed = 2.0;
		this.keys = { LEFT: 'ArrowLeft', UP: 'ArrowUp', RIGHT: 'ArrowRight', BOTTOM: 'ArrowDown' };
		this.mouseButtons = { LEFT: MOUSE.ROTATE, MIDDLE: MOUSE.DOLLY, RIGHT: MOUSE.PAN };
		this.touches = { ONE: TOUCH.ROTATE, TWO: TOUCH.DOLLY_PAN };

		this.target0 = this.target.clone();
		this.position0 = this.object.position.clone();
		this.zoom0 = this.object.zoom;

		this._cursorStyle = 'auto';
		this._domElementKeyEvents = null;
		this._lastPosition = new Vector3();
		this._lastQuaternion = new Quaternion();
		this._lastTargetPosition = new Vector3();
		this._quat = new Quaternion().setFromUnitVectors( object.up, new Vector3( 0, 1, 0 ) );
		this._quatInverse = this._quat.clone().invert();
		this._spherical = new Spherical();
		this._sphericalDelta = new Spherical();
		this._scale = 1;
		this._panOffset = new Vector3();
		this._rotateStart = new Vector2();
		this._rotateEnd = new Vector2();
		this._rotateDelta = new Vector2();
		this._panStart = new Vector2();
		this._panEnd = new Vector2();
		this._panDelta = new Vector2();
		this._dollyStart = new Vector2();
		this._dollyEnd = new Vector2();
		this._dollyDelta = new Vector2();
		this._dollyDirection = new Vector3();
		this._mouse = new Vector2();
		this._performCursorZoom = false;
		this._pointers = [];
		this._pointerPositions = {};
		this._controlActive = false;

		this._onPointerMove = onPointerMove.bind( this );
		this._onPointerDown = onPointerDown.bind( this );
		this._onPointerUp = onPointerUp.bind( this );
		this._onContextMenu = onContextMenu.bind( this );
		this._onMouseWheel = onMouseWheel.bind( this );
		this._onKeyDown = onKeyDown.bind( this );
		this._onTouchStart = onTouchStart.bind( this );
		this._onTouchMove = onTouchMove.bind( this );
		this._onMouseDown = onMouseDown.bind( this );
		this._onMouseMove = onMouseMove.bind( this );
		this._interceptControlDown = interceptControlDown.bind( this );
		this._interceptControlUp = interceptControlUp.bind( this );

		if ( this.domElement !== null ) {
			this.connect( this.domElement );
		}

		this.update();
	}

	set cursorStyle( type ) {
		this._cursorStyle = type;
		this.domElement.style.cursor = type === 'grab' ? 'grab' : 'auto';
	}

	get cursorStyle() {
		return this._cursorStyle;
	}

	connect( element ) {
		super.connect( element );
		this.domElement.addEventListener( 'pointerdown', this._onPointerDown );
		this.domElement.addEventListener( 'pointercancel', this._onPointerUp );
		this.domElement.addEventListener( 'contextmenu', this._onContextMenu );
		this.domElement.addEventListener( 'wheel', this._onMouseWheel, { passive: false } );
		const document = this.domElement.getRootNode();
		document.addEventListener( 'keydown', this._interceptControlDown, { passive: true, capture: true } );
		this.domElement.style.touchAction = 'none';
	}

	disconnect() {
		this.domElement.removeEventListener( 'pointerdown', this._onPointerDown );
		this.domElement.ownerDocument.removeEventListener( 'pointermove', this._onPointerMove );
		this.domElement.ownerDocument.removeEventListener( 'pointerup', this._onPointerUp );
		this.domElement.removeEventListener( 'pointercancel', this._onPointerUp );
		this.domElement.removeEventListener( 'wheel', this._onMouseWheel );
		this.domElement.removeEventListener( 'contextmenu', this._onContextMenu );
		this.stopListenToKeyEvents();
		const document = this.domElement.getRootNode();
		document.removeEventListener( 'keydown', this._interceptControlDown, { capture: true } );
		this.domElement.style.touchAction = 'auto';
	}

	dispose() {
		this.disconnect();
	}

	listenToKeyEvents( domElement ) {
		domElement.addEventListener( 'keydown', this._onKeyDown );
		this._domElementKeyEvents = domElement;
	}

	stopListenToKeyEvents() {
		if ( this._domElementKeyEvents !== null ) {
			this._domElementKeyEvents.removeEventListener( 'keydown', this._onKeyDown );
			this._domElementKeyEvents = null;
		}
	}

	update( deltaTime = null ) {
		const position = this.object.position;
		_v.copy( position ).sub( this.target );
		_v.applyQuaternion( this._quat );
		this._spherical.setFromVector3( _v );

		if ( this.autoRotate && this.state === _STATE.NONE ) {
			this._rotateLeft( this._getAutoRotationAngle( deltaTime ) );
		}

		if ( this.enableDamping ) {
			this._spherical.theta += this._sphericalDelta.theta * this.dampingFactor;
			this._spherical.phi += this._sphericalDelta.phi * this.dampingFactor;
		} else {
			this._spherical.theta += this._sphericalDelta.theta;
			this._spherical.phi += this._sphericalDelta.phi;
		}

		this._spherical.phi = Math.max( this.minPolarAngle, Math.min( this.maxPolarAngle, this._spherical.phi ) );
		this._spherical.makeSafe();

		if ( this.enableDamping === true ) {
			this.target.addScaledVector( this._panOffset, this.dampingFactor );
		} else {
			this.target.add( this._panOffset );
		}

		this.target.sub( this.cursor );
		this.target.clampLength( this.minTargetRadius, this.maxTargetRadius );
		this.target.add( this.cursor );

		let zoomChanged = false;
		const prevRadius = this._spherical.radius;
		this._spherical.radius = this._clampDistance( this._spherical.radius * this._scale );
		zoomChanged = prevRadius !== this._spherical.radius;

		_v.setFromSpherical( this._spherical );
		_v.applyQuaternion( this._quatInverse );
		position.copy( this.target ).add( _v );
		this.object.lookAt( this.target );

		if ( this.enableDamping === true ) {
			this._sphericalDelta.theta *= ( 1 - this.dampingFactor );
			this._sphericalDelta.phi *= ( 1 - this.dampingFactor );
			this._panOffset.multiplyScalar( 1 - this.dampingFactor );
		} else {
			this._sphericalDelta.set( 0, 0, 0 );
			this._panOffset.set( 0, 0, 0 );
		}

		this._scale = 1;
		this._performCursorZoom = false;

		if ( zoomChanged ||
			this._lastPosition.distanceToSquared( this.object.position ) > _EPS ||
			8 * ( 1 - this._lastQuaternion.dot( this.object.quaternion ) ) > _EPS ||
			this._lastTargetPosition.distanceToSquared( this.target ) > _EPS ) {
			this.dispatchEvent( _changeEvent );
			this._lastPosition.copy( this.object.position );
			this._lastQuaternion.copy( this.object.quaternion );
			this._lastTargetPosition.copy( this.target );
			return true;
		}

		return false;
	}

	_getAutoRotationAngle( deltaTime ) {
		if ( deltaTime !== null ) {
			return ( _twoPI / 60 * this.autoRotateSpeed ) * deltaTime;
		}
		return _twoPI / 60 / 60 * this.autoRotateSpeed;
	}

	_getZoomScale( delta ) {
		const normalizedDelta = Math.abs( delta * 0.01 );
		return Math.pow( 0.95, this.zoomSpeed * normalizedDelta );
	}

	_rotateLeft( angle ) {
		this._sphericalDelta.theta -= angle;
	}

	_rotateUp( angle ) {
		this._sphericalDelta.phi -= angle;
	}

	_dollyOut( dollyScale ) {
		this._scale /= dollyScale;
	}

	_dollyIn( dollyScale ) {
		this._scale *= dollyScale;
	}

	_handleMouseWheel( event ) {
		if ( event.deltaY < 0 ) {
			this._dollyIn( this._getZoomScale( event.deltaY ) );
		} else if ( event.deltaY > 0 ) {
			this._dollyOut( this._getZoomScale( event.deltaY ) );
		}
		this.update();
	}

	_customWheelEvent( event ) {
		const newEvent = {
			clientX: event.clientX,
			clientY: event.clientY,
			deltaY: event.deltaY,
		};
		if ( event.ctrlKey && ! this._controlActive ) {
			newEvent.deltaY *= 10;
		}
		return newEvent;
	}

	_clampDistance( dist ) {
		return Math.max( this.minDistance, Math.min( this.maxDistance, dist ) );
	}

	_addPointer( event ) {
		this._pointers.push( event.pointerId );
	}

	_removePointer( event ) {
		delete this._pointerPositions[ event.pointerId ];
		for ( let i = 0; i < this._pointers.length; i ++ ) {
			if ( this._pointers[ i ] == event.pointerId ) {
				this._pointers.splice( i, 1 );
				return;
			}
		}
	}

	_isTrackingPointer( event ) {
		for ( let i = 0; i < this._pointers.length; i ++ ) {
			if ( this._pointers[ i ] == event.pointerId ) return true;
		}
		return false;
	}
}

function onPointerDown( event ) {
	if ( this.enabled === false ) return;
	if ( this._pointers.length === 0 ) {
		this.domElement.setPointerCapture( event.pointerId );
		this.domElement.ownerDocument.addEventListener( 'pointermove', this._onPointerMove );
		this.domElement.ownerDocument.addEventListener( 'pointerup', this._onPointerUp );
	}
	if ( this._isTrackingPointer( event ) ) return;
	this._addPointer( event );
	if ( event.pointerType === 'touch' ) {
		this._onTouchStart( event );
	} else {
		this._onMouseDown( event );
	}
}

function onPointerMove( event ) {
	if ( this.enabled === false ) return;
	this._onMouseMove( event );
}

function onPointerUp( event ) {
	this._removePointer( event );
	if ( this._pointers.length === 0 ) {
		this.domElement.releasePointerCapture( event.pointerId );
		this.domElement.ownerDocument.removeEventListener( 'pointermove', this._onPointerMove );
		this.domElement.ownerDocument.removeEventListener( 'pointerup', this._onPointerUp );
		this.dispatchEvent( _endEvent );
		this.state = _STATE.NONE;
	}
}

function onMouseDown( event ) {
	if ( event.button !== 0 || this.enableRotate === false ) {
		this.state = _STATE.NONE;
		return;
	}

	this._rotateStart.set( event.clientX, event.clientY );
	this.state = _STATE.ROTATE;
	this.dispatchEvent( _startEvent );
}

function onMouseMove( event ) {
	if ( this.state !== _STATE.ROTATE || this.enableRotate === false ) return;

	this._rotateEnd.set( event.clientX, event.clientY );
	this._rotateDelta.subVectors( this._rotateEnd, this._rotateStart ).multiplyScalar( this.rotateSpeed );

	const element = this.domElement;
	this._rotateLeft( _twoPI * this._rotateDelta.x / element.clientHeight );
	this._rotateUp( _twoPI * this._rotateDelta.y / element.clientHeight );

	this._rotateStart.copy( this._rotateEnd );
	this.update();
}

function onMouseWheel( event ) {
	if ( this.enabled === false || this.enableZoom === false || this.state !== _STATE.NONE ) return;
	event.preventDefault();
	this.dispatchEvent( _startEvent );
	this._handleMouseWheel( this._customWheelEvent( event ) );
	this.dispatchEvent( _endEvent );
}

function onKeyDown() {}
function onTouchStart() {}
function onTouchMove() {}

function onContextMenu( event ) {
	if ( this.enabled === false ) return;
	event.preventDefault();
}

function interceptControlDown( event ) {
	if ( event.key === 'Control' ) {
		this._controlActive = true;
		const document = this.domElement.getRootNode();
		document.addEventListener( 'keyup', this._interceptControlUp, { passive: true, capture: true } );
	}
}

function interceptControlUp( event ) {
	if ( event.key === 'Control' ) {
		this._controlActive = false;
		const document = this.domElement.getRootNode();
		document.removeEventListener( 'keyup', this._interceptControlUp, { passive: true, capture: true } );
	}
}

export { OrbitControls };

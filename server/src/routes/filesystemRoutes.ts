import { Router } from 'express';
import { FileSystemController } from '../controllers/filesystemController';
import { Express } from 'express-serve-static-core';

const router = Router();
const filesystemController = new FileSystemController();

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/list', filesystemController.list);

    router.post('/api/mkdir/:name', filesystemController.mkdir);
    router.delete('/api/rmdir/:name', filesystemController.rmdir);
}